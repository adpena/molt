//! Shared cached SimpleIR -> TIR optimization runner.
//!
//! Every backend uses this authority for cache keys, batching, hit restoration,
//! miss optimization, artifact encoding, and index persistence. Every caller
//! borrows source declarations and receives optimized TIR.
//! Backend-specific code may prepare a function before lowering or consume the
//! optimized [`TirFunction`] afterward, but it must not open `CompilationCache`
//! directly for this pipeline.

use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use molt_ir::deterministic_hash::HashSet;
use rayon::prelude::*;
use std::borrow::{Borrow, Cow};
use std::collections::BTreeMap;

use crate::FunctionIR;

use molt_ir::content_digest::CompilationCacheKey;
use molt_ir::ir::FunctionIrContractDigest;

use super::cache::{CompilationCache, CompilationCacheWriteError};
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
    FactGraph,
}

impl TirPipelineCacheFlavor {
    fn cache_discriminant(self) -> &'static [u8] {
        match self {
            Self::Native => b"native",
            Self::Llvm => b"llvm",
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

pub struct CachedTirCustody {
    // All bodies in this result were optimized under these conditions.
    // Partitioning and module passes use the same authority.
    target_info: TargetInfo,
    target_contract: String,
    optimized_tir_by_name: BTreeMap<String, TirFunction>,
    finalization_cache: Option<CompilationCache>,
}

impl std::fmt::Debug for CachedTirCustody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CachedTirCustody")
            .field("target_contract", &self.target_contract)
            .field("optimized_tir_by_name", &self.optimized_tir_by_name)
            .field("finalization_cache", &self.finalization_cache.is_some())
            .finish()
    }
}

impl CachedTirCustody {
    fn new(target_info: TargetInfo) -> Self {
        let target_contract = tir_pipeline_target_fingerprint(&target_info);
        Self {
            target_info,
            target_contract,
            optimized_tir_by_name: BTreeMap::new(),
            finalization_cache: None,
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

    pub fn contains_function(&self, name: &str) -> bool {
        self.optimized_tir_by_name.contains_key(name)
    }
}

#[derive(Debug)]
pub struct TirPipelineRun {
    pub cached_tir: CachedTirCustody,
    pub uncached_count: usize,
}

/// Where the persistent TIR cache lives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TirCacheLocation {
    /// The running backend binary's namespace under the process's cache root,
    /// `MOLT_CACHE`, which molt passes to every backend process. Without it
    /// there is no persistent cache, never one in the current directory.
    ProcessRoot,
    /// The running backend binary's namespace under this root. The JIT
    /// runner's root comes from its image: the runner's environment and
    /// current directory are the program's.
    Root(std::path::PathBuf),
    /// Exactly this directory, with no namespace.
    Directory(std::path::PathBuf),
}

pub struct TirPipelineRunOptions<'a> {
    /// Known contracts in the input document's row order. A preparation
    /// that rewrites the body always computes its new contract.
    pub source_contracts: Option<&'a [Option<FunctionIrContractDigest>]>,
    pub target_info: TargetInfo,
    pub cache_flavor: TirPipelineCacheFlavor,
    pub cache: TirCacheLocation,
    pub process_externs: bool,
    pub tir_dump: bool,
    pub tir_stats: bool,
    pub progress_prefix: Option<&'a str>,
    pub resource_plan: TirOptimizationResourcePlan,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TirOwnedModulePipelineMode {
    ModulePhase,
    TerminalDropsOnly,
}

pub struct TirOwnedModulePipelineOptions<'a> {
    /// Present only when this job owns and can resize the emitted program storage.
    pub cell_registry: Option<&'a mut molt_ir::ModuleRegistryIR>,
    pub module_name: &'a str,
    pub non_inlinable: &'a HashSet<String>,
    pub missing_tir_context: &'a str,
    pub mode: TirOwnedModulePipelineMode,
    /// The program's linkage rows, for backends that consume final TIR; the
    /// late lowering stage admits guarded direct calls against them.
    pub linkage: Option<&'a molt_ir::linkage_abi::FunctionSignatures>,
}

struct PreparedTirFunction<'a> {
    function: Cow<'a, FunctionIR>,
    content_hash: String,
}

impl<'a> PreparedTirFunction<'a> {
    fn new<F>(
        source: &'a FunctionIR,
        table: &molt_ir::FunctionTable,
        cache_prefix: &CompilationCacheKey,
        source_contract: Option<FunctionIrContractDigest>,
        preprocess_before_lowering: &F,
    ) -> Self
    where
        F: Fn(&mut Cow<'_, FunctionIR>) + ?Sized,
    {
        // Most preparations only inspect the source. Retain its admitted body
        // on cache hits (and misses), copying only if preparation writes it.
        let mut function = Cow::Borrowed(source);
        trace_tir_function_stage(&function.name, "start", function.ops.len());
        preprocess_before_lowering(&mut function);
        let content_hash = if let (Cow::Borrowed(borrowed), Some(contract)) =
            (&function, source_contract)
            && std::ptr::eq(*borrowed, source)
        {
            let mut key = cache_prefix.clone();
            key.precomputed_digest_field(b"function-ir-contract", contract.as_bytes());
            key.finish_hex()
        } else {
            function_content_hash(&function, table, cache_prefix)
        };
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

struct TirOptimizationInput<'a> {
    index: usize,
    prepared_function: PreparedTirFunction<'a>,
}

struct TirOptimizationOutput {
    index: usize,
    content_hash: String,
    tir_func: TirFunction,
}

pub fn content_hash_for_function(
    func_ir: &FunctionIR,
    table: &molt_ir::FunctionTable,
    cache_flavor: TirPipelineCacheFlavor,
    target_info: &TargetInfo,
) -> String {
    function_content_hash(
        func_ir,
        table,
        &preparation_key_prefix(cache_flavor, &tir_pipeline_target_fingerprint(target_info)),
    )
}

fn preparation_key_prefix(
    cache_flavor: TirPipelineCacheFlavor,
    target_contract: &str,
) -> CompilationCacheKey {
    let mut key = CompilationCacheKey::new(b"molt-cached-tir-prepared-function-key-v1");
    key.field(b"pipeline-semantic-epoch", TIR_PIPELINE_SEMANTIC_EPOCH);
    key.field(b"cache-flavor", cache_flavor.cache_discriminant());
    key.field(b"target-contract", target_contract.as_bytes());
    key
}

fn function_content_hash(
    func_ir: &FunctionIR,
    table: &molt_ir::FunctionTable,
    prefix: &CompilationCacheKey,
) -> String {
    let mut key = prefix.clone();
    key.digest_field(b"function-ir-contract", |writer| {
        crate::write_function_ir_contract(func_ir, table, writer)
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
    let super::target_info::SimpleIrLoweringFeatures {
        cold_origin_frame_location,
        final_lowering,
        module_global_slots,
        method_sites,
        list_reverse_slice,
    } = lowering;
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
            "runtime_requirements={};lowering_cold_origin_location={};lowering_final={};lowering_global_slots={};lowering_method_sites={};lowering_list_reverse_slice={};",
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
        cold_origin_frame_location,
        final_lowering.as_str(),
        module_global_slots,
        method_sites,
        list_reverse_slice,
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

static COMPILE_THREAD_POOLS: std::sync::Mutex<Vec<(usize, std::sync::Arc<rayon::ThreadPool>)>> =
    std::sync::Mutex::new(Vec::new());

/// Compiler pool threads that have not exited yet. A pool counts its threads
/// when it is built; each thread uncounts itself on its way out.
static LIVE_COMPILE_THREADS: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

fn compile_thread_exited(_index: usize) {
    let (count, exited) = &LIVE_COMPILE_THREADS;
    let mut count = count
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *count = count.saturating_sub(1);
    exited.notify_all();
}

/// The compiler pool with exactly `threads` workers, shared by every compile
/// in this process. A daemon running several compiles at once then keeps one
/// thread per planned worker instead of one pool per compile: the bound that
/// the make/Cargo jobserver gives concurrent rustc processes. The least
/// recently used pool leaves when a new width needs a slot; its threads exit
/// once the compiles that hold it finish.
pub fn compile_thread_pool(threads: usize) -> std::sync::Arc<rayon::ThreadPool> {
    let threads = threads.max(1);
    let mut pools = COMPILE_THREAD_POOLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let pool = match pools.iter().position(|(width, _)| *width == threads) {
        Some(index) => pools.remove(index).1,
        None => {
            if pools.len() == MAX_COMPILE_THREAD_POOLS {
                pools.remove(0);
            }
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .stack_size(COMPILE_THREAD_STACK_BYTES)
                .thread_name(|index| format!("molt-compile-{index}"))
                .exit_handler(compile_thread_exited)
                .build()
                .expect("Failed to build compiler thread pool");
            *LIVE_COMPILE_THREADS
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += threads;
            std::sync::Arc::new(pool)
        }
    };
    pools.push((threads, std::sync::Arc::clone(&pool)));
    pool
}

/// Drop every compiler pool and wait until each of their threads has exited.
///
/// A process that compiles and then runs other code in itself (the JIT
/// runner, `runtime/molt-jit`) calls this between the two, so the code it
/// runs starts with no compiler threads beside it, as an AOT executable does.
/// The caller must hold no pool: a held pool keeps its threads alive and this
/// call would wait for them forever.
pub fn retire_compile_thread_pools() {
    COMPILE_THREAD_POOLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    let (count, exited) = &LIVE_COMPILE_THREADS;
    let mut count = count
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while *count > 0 {
        count = exited
            .wait(count)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
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
    functions: &[FunctionIR],
    options: TirPipelineRunOptions<'_>,
    preprocess_before_lowering: F,
) -> TirPipelineRun
where
    F: Fn(&mut Cow<'_, FunctionIR>) + Sync,
{
    // Reusing optimized TIR would suppress pass dumps, audits and instruments.
    let cache_reuse_allowed = !options.tir_dump
        && !options.tir_stats
        && !molt_ir::backend_environment::compilation_diagnostics_requested();
    let mut cached_tir_custody = CachedTirCustody::new(options.target_info);
    let mut tir_cache = match &options.cache {
        TirCacheLocation::ProcessRoot => CompilationCache::open_backend_namespace(),
        TirCacheLocation::Root(root) => {
            Some(CompilationCache::open_backend_namespace_under(root.clone()))
        }
        TirCacheLocation::Directory(directory) => Some(CompilationCache::open(directory.clone())),
    };
    // The document table: digests and cached bodies name callees by symbol,
    // and restore relocates them to this document's ids (I5).
    let table = molt_ir::FunctionTable::of(functions);
    let symbol_ids: molt_ir::deterministic_hash::HashMap<String, molt_ir::FunctionId> = functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            (
                function.name.clone(),
                molt_ir::FunctionId::from_index(index),
            )
        })
        .collect();
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
        let cache_prefix =
            preparation_key_prefix(options.cache_flavor, &cached_tir_custody.target_contract);
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
                                &functions[work_item.index],
                                &table,
                                &cache_prefix,
                                options
                                    .source_contracts
                                    .and_then(|rows| rows[work_item.index]),
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
            // A wave bounds both the parallel reads and live projections.
            // The cache admits each envelope before its worker restores and
            // verifies the body against this program's symbol table.
            let mut cached_bodies = tir_cache
                .as_mut()
                .filter(|_| cache_reuse_allowed)
                .map(|cache| {
                    let keys: Vec<_> = inputs
                        .iter()
                        .map(|input| input.prepared_function.content_hash.as_str())
                        .collect();
                    cache.read_many(
                        &keys,
                        |payload| {
                            super::serialize::deserialize_tir_function(payload, &|symbol| {
                                symbol_ids.get(symbol).copied()
                            })
                        },
                        |indices, read| {
                            if indices.len() <= 1 {
                                return indices.map(read).collect();
                            }
                            tir_pool.install(|| {
                                indices
                                    .into_par_iter()
                                    .map(|index| request.enter(|| read(index)))
                                    .collect()
                            })
                        },
                    )
                })
                .into_iter()
                .flatten()
                .map(Option::flatten);
            let mut misses = Vec::new();
            for prepared in inputs {
                let index = prepared.index;
                if let Some(cached_tir_func) = cached_bodies.next().flatten() {
                    let func_ir = &functions[index];
                    assert_eq!(
                        func_ir.return_abi, cached_tir_func.return_abi,
                        "cached TIR must preserve the authored function return ABI"
                    );
                    let name = func_ir.name.clone();
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
                                &cached_tir_custody.target_info,
                                options.tir_dump,
                                options.tir_stats,
                            )
                        })
                    })
                    .collect()
            });

            for output in results {
                let func_ir = &functions[output.index];
                assert_eq!(
                    func_ir.return_abi, output.tir_func.return_abi,
                    "optimized TIR must preserve the authored function return ABI"
                );
                let name = func_ir.name.clone();
                let bytes = super::serialize::serialize_tir_function(&output.tir_func, &table)
                    .unwrap_or_else(|error| {
                        panic!(
                            "cached TIR serialization failed for '{}': {error}",
                            output.tir_func.name
                        )
                    });
                if let Some(Err(error)) = tir_cache
                    .as_mut()
                    .map(|cache| cache.put(&output.content_hash, &bytes))
                {
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
    if cache_reuse_allowed {
        cached_tir_custody.finalization_cache = tir_cache;
    }

    TirPipelineRun {
        cached_tir: cached_tir_custody,
        uncached_count,
    }
}

/// Whether `body` holds an operation SimpleIR cannot express: one the late
/// lowering stage, or a final-TIR target's mid pipeline, introduced.
fn has_final_tir_operations(body: &TirFunction) -> bool {
    use molt_ir::tir::op_kinds_generated::opcode_canonical_kind_table;
    body.blocks
        .values()
        .flat_map(|block| &block.ops)
        .any(|op| opcode_canonical_kind_table(op.opcode).is_none())
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
    max_ops: usize,
) -> BTreeMap<String, String> {
    // Final-TIR consumers never project ordinary functions. Only the structural
    // splitter still consumes SimpleIR, before terminal drops and late expansions.
    let mut needs_split = false;
    for function in ir
        .functions
        .iter_mut()
        .filter(|function| !function.is_extern)
    {
        // The cached body owns the executable program. A refused split must
        // retain that body, without keeping the source or its stale callees.
        function.ops = Vec::new();
        let body = &cached_tir.optimized_tir_by_name[&function.name];
        if body.operation_count() > max_ops {
            // A final-TIR target's mid pipeline may already emit operations
            // with no SimpleIR form (checked_int_loops' inline-int entry
            // guard). The splitter cannot see such a body; it stays whole,
            // as a refused split does, and the build records why.
            if has_final_tir_operations(body) {
                trace_tir_function_stage(
                    &function.name,
                    "split_refused_final_tir_operations",
                    body.operation_count(),
                );
                continue;
            }
            let projection = super::lower_to_simple::lower_to_simple_ir(body);
            if projection.len() > max_ops {
                function.ops = projection;
                needs_split = true;
            }
        }
    }
    if !needs_split {
        return BTreeMap::new();
    }
    let sources = crate::passes::split_megafunctions_at_limit(ir, max_ops, |f| !f.ops.is_empty());
    let changed: HashSet<&str> = sources
        .keys()
        .chain(sources.values())
        .map(String::as_str)
        .collect();
    for function in &mut ir.functions {
        if changed.contains(function.name.as_str()) {
            // Only stubs and chunks acquired new SSA boundaries. Use their
            // new parameter and dominance facts before terminal drop insertion.
            let target_info = &cached_tir.target_info;
            let mut tir = super::lower_from_simple::lower_to_tir_for_target(function, target_info);
            super::type_refine::refine_types(&mut tir);
            super::passes::run_pipeline(&mut tir, target_info);
            super::type_refine::refine_types(&mut tir);
            cached_tir.insert(function.name.clone(), tir);
        }
        function.ops = Vec::new();
    }
    sources
}

/// The final bodies of a document, one per row in row order (`is_extern`,
/// body), and the specialized clones the module phase appended after the
/// source rows (design 78, slice 3).
pub struct TirOwnedModule {
    pub functions: Vec<(bool, TirFunction)>,
    pub clones: Vec<super::passes::type_flow::SpecializedClone>,
}

impl TirOwnedModule {
    /// Give each clone the source row of its origin under the clone's
    /// symbol, so `functions` (the document's SimpleIR rows) stays aligned
    /// with [`Self::functions`]. A clone is another compiled entry of its
    /// origin: the same parameters, custody and linkage signature.
    pub fn append_clone_rows(&self, functions: &mut Vec<FunctionIR>) {
        for clone in &self.clones {
            assert_eq!(
                functions.len(),
                clone.id.index(),
                "clone rows follow the document's rows in order"
            );
            let origin = &mut functions[clone.origin.index()];
            let ops = std::mem::take(&mut origin.ops);
            let mut row = origin.clone();
            origin.ops = ops;
            row.name = self.functions[clone.id.index()].1.name.clone();
            functions.push(row);
        }
    }
}

/// Consume source bodies during assembly when the caller owns them; borrowed
/// sources (for diagnostics) stay untouched. No local SimpleIR ops are read.
pub fn run_owned_module_pipeline_from_cached_tir<F: Borrow<FunctionIR>>(
    functions: impl IntoIterator<Item = F>,
    cached_tir: &mut CachedTirCustody,
    options: TirOwnedModulePipelineOptions<'_>,
) -> TirOwnedModule {
    let mut clones = Vec::new();
    let mut tir_functions = take_ordered_tir_functions_from_cached_tir(
        functions,
        cached_tir,
        options.missing_tir_context,
    );
    let target_info = &cached_tir.target_info;
    // Finalization consumes this custody. Retire the resident cache before
    // LLVM/Cranelift codegen starts retaining its own module-sized products.
    let mut finalization_store = cached_tir.finalization_cache.take();
    let mut finalization_cache = finalization_store
        .as_mut()
        .map(|store| super::drop_phase::FinalizationCache::new(store, &cached_tir.target_contract));
    let table = molt_ir::FunctionTable::from_rows(tir_functions.iter().map(|(is_extern, func)| {
        let kind = if *is_extern {
            molt_ir::function_table::FunctionRowKind::Extern
        } else {
            molt_ir::function_table::FunctionRowKind::Body
        };
        (func.name.as_str(), kind)
    }));
    match options.mode {
        TirOwnedModulePipelineMode::ModulePhase => {
            let mut rows: Vec<Option<(bool, TirFunction)>> =
                Vec::with_capacity(tir_functions.len());
            let mut bodies = Vec::new();
            for (index, (is_extern, tir_func)) in tir_functions.into_iter().enumerate() {
                if is_extern {
                    rows.push(Some((true, tir_func)));
                } else {
                    rows.push(None);
                    bodies.push((molt_ir::FunctionId::from_index(index), tir_func));
                }
            }
            let mut module = TirModule::from_document(options.module_name, table, bodies);
            let outcome = super::module_phase::run_module_pipeline(
                &mut module,
                target_info,
                options.non_inlinable,
                options.linkage,
                options.cell_registry,
                finalization_cache.as_mut(),
            );
            // Specialized clones are rows after the source rows.
            rows.resize_with(module.table.len(), || None);
            clones = outcome.clones;
            // Document order: the result's position is each function's id.
            for (id, func) in module.ids.into_iter().zip(module.functions) {
                rows[id.index()] = Some((false, func));
            }
            tir_functions = rows
                .into_iter()
                .map(|row| row.expect("every row returns from the module phase"))
                .collect();
        }
        TirOwnedModulePipelineMode::TerminalDropsOnly => {
            super::drop_phase::finalize_document_drops(
                tir_functions
                    .iter_mut()
                    .enumerate()
                    .filter_map(|(index, (external, body))| {
                        (!*external).then_some((molt_ir::FunctionId::from_index(index), body))
                    }),
                target_info,
                options.linkage,
                &table,
                finalization_cache.as_mut(),
            );
        }
    }
    TirOwnedModule {
        functions: tir_functions,
        clones,
    }
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

fn optimize_tir_input(
    input: TirOptimizationInput,
    target_info: &TargetInfo,
    tir_dump: bool,
    tir_stats: bool,
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
    TirOptimizationOutput {
        index: idx,
        content_hash,
        tir_func,
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
    use crate::OpIR;

    fn projection_fixture() -> Vec<FunctionIR> {
        let constant = |name: &str, value| FunctionIR {
            name: name.into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            source_file: Some("projection.py".into()),
            execution_context: crate::ExecutionContextPolicy::Inherited,
            ops: vec![
                OpIR {
                    kind: molt_ir::OpKind::Const,
                    value: Some(value),
                    out: Some("authored".into()),
                    ..OpIR::new(molt_ir::OpKind::Const)
                },
                OpIR {
                    kind: molt_ir::OpKind::Ret,
                    args: Some(vec!["authored".into()]),
                    ..OpIR::new(molt_ir::OpKind::Ret)
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
            source_contracts: None,
            target_info: TargetInfo::native_release_fast(),
            cache_flavor: TirPipelineCacheFlavor::FactGraph,
            cache: TirCacheLocation::Directory(cache_dir.to_path_buf()),
            process_externs: false,
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
                    super::super::serialize::serialize_tir_function(tir, &Default::default())
                        .unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn body_edit_reuses_unchanged_tir_without_copying_prepared_sources() {
        let cache_dir =
            std::env::temp_dir().join(format!("molt-tir-borrowed-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let mut source = projection_fixture();
        let run = |source: &[FunctionIR], directory: &std::path::Path| {
            run_cached_tir_pipeline(source, projection_test_options(directory), |prepared| {
                // Source identity is an allocation oracle: even a cache hit
                // must reach preparation without materializing another body.
                let authored = source.iter().find(|row| row.name == prepared.name).unwrap();
                assert!(std::ptr::eq(prepared.as_ref(), authored));
            })
        };
        let cold = run(&source, &cache_dir);
        assert_eq!(cold.uncached_count, 2);
        let cold_bytes = cached_body_bytes(&cold);
        assert_eq!(run(&source, &cache_dir).uncached_count, 0);

        source[0].ops[0].value = Some(42);
        let edited = run(&source, &cache_dir);
        assert_eq!(edited.uncached_count, 1);
        let edited_bytes = cached_body_bytes(&edited);
        assert_ne!(edited_bytes[0], cold_bytes[0]);
        assert_eq!(edited_bytes[1], cold_bytes[1]);
        // Fresh optimization is the semantic oracle for the changed program.
        let fresh = run(&source, &cache_dir.join("fresh"));
        assert_eq!(fresh.uncached_count, 2);
        assert_eq!(edited_bytes, cached_body_bytes(&fresh));
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn cold_and_warm_cache_return_identical_tir_and_preserve_source() {
        let cache_dir =
            std::env::temp_dir().join(format!("molt-tir-projection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let source = projection_fixture();
        let source_bytes = serde_json::to_vec(&source).unwrap();
        let direct = run_cached_tir_pipeline(&source, projection_test_options(&cache_dir), |_| {});
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
        let direct_hit =
            run_cached_tir_pipeline(&source, projection_test_options(&cache_dir), |_| {});
        assert_eq!(direct_hit.uncached_count, 0);
        assert_eq!(cached_body_bytes(&direct_hit), expected_tir);
        // A diagnostic request executes the passes even after a cache hit.
        let mut diagnostics = projection_test_options(&cache_dir);
        diagnostics.tir_stats = true;
        let diagnostic = run_cached_tir_pipeline(&source, diagnostics, |_| {});
        assert_eq!(diagnostic.uncached_count, 2);
        assert_eq!(cached_body_bytes(&diagnostic), expected_tir);
        for mode in [
            TirOwnedModulePipelineMode::ModulePhase,
            TirOwnedModulePipelineMode::TerminalDropsOnly,
        ] {
            let directory = cache_dir.join(format!("{mode:?}"));
            let non_inlinable = HashSet::default();
            let finish = |mut run: TirPipelineRun, diagnostics: bool| {
                assert_eq!(run.cached_tir.finalization_cache.is_some(), !diagnostics);
                let owned = run_owned_module_pipeline_from_cached_tir(
                    &source,
                    &mut run.cached_tir,
                    TirOwnedModulePipelineOptions {
                        cell_registry: None,
                        module_name: "projection_fixture",
                        non_inlinable: &non_inlinable,
                        missing_tir_context: "cold/warm final body verification",
                        mode,
                        linkage: None,
                    },
                );
                assert!(run.cached_tir.finalization_cache.is_none());
                owned
                    .functions
                    .iter()
                    .filter(|(external, _)| !*external)
                    .map(|(_, body)| {
                        super::super::serialize::serialize_tir_function(
                            body,
                            &molt_ir::FunctionTable::of(&source),
                        )
                        .unwrap()
                    })
                    .collect::<Vec<_>>()
            };
            let run = |diagnostics| {
                let mut options = projection_test_options(&directory);
                options.tir_stats = diagnostics;
                finish(
                    run_cached_tir_pipeline(&source, options, |_| {}),
                    diagnostics,
                )
            };
            let cold = run(false);
            assert_eq!(run(false), cold);
            assert_eq!(run(true), cold);
        }
        assert_eq!(serde_json::to_vec(&source).unwrap(), source_bytes);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn cached_and_optimized_bodies_must_preserve_the_authored_return_abi() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        for cached in [false, true] {
            let cache_dir = std::env::temp_dir().join(format!(
                "molt-tir-return-abi-{}-{cached}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&cache_dir);
            let source = vec![FunctionIR {
                name: "authored_void".into(),
                return_abi: molt_ir::FunctionReturnAbi::Void,
                ops: vec![OpIR {
                    kind: molt_ir::OpKind::RetVoid,
                    ..OpIR::new(molt_ir::OpKind::RetVoid)
                }],
                ..FunctionIR::default()
            }];
            let before = serde_json::to_vec(&source).unwrap();
            if cached {
                let options = projection_test_options(&cache_dir);
                let key = content_hash_for_function(
                    &source[0],
                    &molt_ir::FunctionTable::of(&source),
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
                        &super::super::serialize::serialize_tir_function(
                            &wrong,
                            &Default::default(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
            }
            let error = catch_unwind(AssertUnwindSafe(|| {
                run_cached_tir_pipeline(&source, projection_test_options(&cache_dir), |function| {
                    if !cached {
                        function.to_mut().return_abi = molt_ir::FunctionReturnAbi::Value;
                        function.to_mut().ops = projection_fixture().remove(0).ops;
                    }
                })
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
        let non_inlinable = HashSet::default();
        let cache = || {
            let mut cache = CachedTirCustody::new(target_info.clone());
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
                        super::super::serialize::serialize_tir_function(f, &Default::default())
                            .unwrap(),
                    )
                })
                .collect::<Vec<_>>()
        };
        for mode in [
            TirOwnedModulePipelineMode::TerminalDropsOnly,
            TirOwnedModulePipelineMode::ModulePhase,
        ] {
            let options = || TirOwnedModulePipelineOptions {
                cell_registry: None,
                module_name: "owned_source",
                non_inlinable: &non_inlinable,
                missing_tir_context: "owned source test",
                mode,
                linkage: None,
            };
            let borrowed =
                run_owned_module_pipeline_from_cached_tir(&functions, &mut cache(), options())
                    .functions;
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
            let owned = run_owned_module_pipeline_from_cached_tir(sources, &mut cache(), options())
                .functions;
            assert_eq!(drops.get(), 3);
            assert_eq!(bytes(&owned), bytes(&borrowed));
            // Both modes return document order: a position is a FunctionId.
            let _ = mode;
            let expected = vec!["first", "external", "last"];
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
    fn cached_partition_preserves_unmodified_tir_and_plans_every_new_transport() {
        for target in [
            TargetInfo::native_release_fast(),
            TargetInfo::llvm_release_fast(),
        ] {
            let large = FunctionIR {
                name: "split_transport_owner".into(),
                execution_context: crate::ExecutionContextPolicy::Inherited,
                return_abi: molt_ir::FunctionReturnAbi::Value,
                params: vec!["input".into()],
                ops: vec![
                    OpIR {
                        kind: molt_ir::OpKind::Line,
                        value: Some(1),
                        ..OpIR::new(molt_ir::OpKind::Line)
                    },
                    OpIR {
                        kind: molt_ir::OpKind::ListNew,
                        args: Some(vec!["input".into()]),
                        out: Some("kept".into()),
                        ..OpIR::new(molt_ir::OpKind::ListNew)
                    },
                    OpIR {
                        kind: molt_ir::OpKind::Line,
                        value: Some(2),
                        ..OpIR::new(molt_ir::OpKind::Line)
                    },
                    OpIR {
                        kind: molt_ir::OpKind::ListNew,
                        args: Some(vec!["kept".into()]),
                        out: Some("outer".into()),
                        ..OpIR::new(molt_ir::OpKind::ListNew)
                    },
                    OpIR {
                        kind: molt_ir::OpKind::Ret,
                        args: Some(vec!["outer".into()]),
                        ..OpIR::new(molt_ir::OpKind::Ret)
                    },
                ],
                ..FunctionIR::default()
            };
            let unchanged = FunctionIR {
                name: "unchanged".into(),
                return_abi: molt_ir::FunctionReturnAbi::Value,
                params: vec!["arg".into()],
                ops: vec![OpIR {
                    kind: molt_ir::OpKind::Ret,
                    args: Some(vec!["arg".into()]),
                    ..OpIR::new(molt_ir::OpKind::Ret)
                }],
                ..FunctionIR::default()
            };
            let mut ir = crate::SimpleIR {
                functions: vec![large, unchanged],
                profile: None,
            };
            crate::validate_simple_ir(&ir).expect("valid source before outlining");
            let mut cache = CachedTirCustody::new(target.clone());
            for function in &ir.functions {
                cache.insert(
                    function.name.clone(),
                    super::super::lower_from_simple::lower_to_tir_for_target(function, &target),
                );
            }
            let unchanged_params = cache.optimized_tir_by_name["unchanged"]
                .param_names
                .as_ptr();
            // The source row is metadata only. It cannot supply the optimized
            // definition's size, ops or ids to the splitter.
            ir.functions[0].ops.clear();
            let sources = partition_cached_functions_before_drops(&mut ir, &mut cache, 3);
            assert!(ir.functions.iter().all(|f| f.ops.is_empty()));
            assert!(sources.keys().any(|chunk| {
                cache.optimized_tir_by_name[chunk]
                    .blocks
                    .values()
                    .flat_map(|block| &block.ops)
                    .any(|op| op.opcode == super::super::ops::OpCode::Index)
            }));
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
            let signatures = molt_ir::linkage_abi::document_signatures(&ir.functions);
            let final_module = run_owned_module_pipeline_from_cached_tir(
                ir.functions,
                &mut cache,
                TirOwnedModulePipelineOptions {
                    cell_registry: None,
                    module_name: "split_test",
                    non_inlinable: &HashSet::default(),
                    missing_tir_context: "partition test",
                    mode: TirOwnedModulePipelineMode::TerminalDropsOnly,
                    linkage: Some(&signatures),
                },
            );
            for (_, body) in final_module.functions {
                assert_eq!(
                    body.attrs
                        .get(super::super::passes::drop_insertion::DROP_INSERTED_ATTR),
                    Some(&super::super::ops::AttrValue::Bool(true)),
                    "{} must receive its own terminal drop plan",
                    body.name
                );
            }
        }
    }

    #[test]
    fn over_limit_body_with_final_tir_operations_stays_whole() {
        // checked_int_loops on a final-TIR target guards a loop with
        // IsInlineInt, which has no SimpleIR form; projecting it for the
        // splitter panicked (the W3 switch made native a final-TIR target).
        let target = TargetInfo::native_release_fast();
        let function = FunctionIR {
            name: "guarded_megafunction".into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            params: vec!["input".into()],
            ops: vec![
                OpIR {
                    kind: molt_ir::OpKind::ListNew,
                    args: Some(vec!["input".into()]),
                    out: Some("kept".into()),
                    ..OpIR::new(molt_ir::OpKind::ListNew)
                },
                OpIR {
                    kind: molt_ir::OpKind::ListNew,
                    args: Some(vec!["kept".into()]),
                    out: Some("outer".into()),
                    ..OpIR::new(molt_ir::OpKind::ListNew)
                },
                OpIR {
                    kind: molt_ir::OpKind::Ret,
                    args: Some(vec!["outer".into()]),
                    ..OpIR::new(molt_ir::OpKind::Ret)
                },
            ],
            ..FunctionIR::default()
        };
        let mut ir = crate::SimpleIR {
            functions: vec![function],
            profile: None,
        };
        let mut body =
            super::super::lower_from_simple::lower_to_tir_for_target(&ir.functions[0], &target);
        let entry = body.entry_block;
        let input = body.blocks[&entry].args[0].id;
        let guard = body.fresh_value();
        body.blocks.get_mut(&entry).unwrap().ops.insert(
            0,
            super::super::ops::TirOp::new(
                super::super::ops::OpCode::IsInlineInt,
                vec![input],
                vec![guard],
            ),
        );
        assert!(has_final_tir_operations(&body));
        let before = body.operation_count();
        let mut cache = CachedTirCustody::new(target);
        cache.insert("guarded_megafunction".into(), body);
        let sources = partition_cached_functions_before_drops(&mut ir, &mut cache, 1);
        assert!(
            sources.is_empty(),
            "a body SimpleIR cannot express is not split"
        );
        assert!(ir.functions[0].ops.is_empty());
        assert_eq!(
            cache.optimized_tir_by_name["guarded_megafunction"].operation_count(),
            before
        );
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
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "f".to_string(),
            params: vec!["x".to_string()],
            ops: vec![OpIR {
                kind: molt_ir::OpKind::ConstInt,
                out: Some("x".to_string()),
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::ConstInt)
            }],
            param_types: Some(vec!["int".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: crate::ExecutionContextPolicy::None,
        };

        let native = TargetInfo::native_release_fast();
        let llvm = TargetInfo::llvm_release_fast();

        assert_ne!(
            tir_pipeline_target_fingerprint(&native),
            tir_pipeline_target_fingerprint(&llvm)
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
        assert_ne!(
            content_hash_for_function(
                &func,
                &molt_ir::FunctionTable::default(),
                TirPipelineCacheFlavor::Native,
                &native
            ),
            content_hash_for_function(
                &func,
                &molt_ir::FunctionTable::default(),
                TirPipelineCacheFlavor::Llvm,
                &llvm
            )
        );
        assert_ne!(
            content_hash_for_function(
                &func,
                &molt_ir::FunctionTable::default(),
                TirPipelineCacheFlavor::Native,
                &native
            ),
            content_hash_for_function(
                &func,
                &molt_ir::FunctionTable::default(),
                TirPipelineCacheFlavor::FactGraph,
                &native
            )
        );

        let without_source = content_hash_for_function(
            &func,
            &molt_ir::FunctionTable::default(),
            TirPipelineCacheFlavor::FactGraph,
            &native,
        );
        func.source_file = Some("app.py".to_string());
        let with_source = content_hash_for_function(
            &func,
            &molt_ir::FunctionTable::default(),
            TirPipelineCacheFlavor::FactGraph,
            &native,
        );
        func.source_file = Some("other.py".to_string());
        let with_other_source = content_hash_for_function(
            &func,
            &molt_ir::FunctionTable::default(),
            TirPipelineCacheFlavor::FactGraph,
            &native,
        );

        assert_ne!(without_source, with_source);
        assert_ne!(with_source, with_other_source);

        let baseline = func.clone();
        let baseline_hash = content_hash_for_function(
            &baseline,
            &molt_ir::FunctionTable::default(),
            TirPipelineCacheFlavor::Native,
            &native,
        );
        let mut contract_mutations = Vec::new();
        let mut changed = baseline.clone();
        changed.return_abi = molt_ir::FunctionReturnAbi::Value;
        contract_mutations.push(changed);
        let mut changed = baseline.clone();
        changed.params.push("y".to_string());
        contract_mutations.push(changed);
        let mut changed = baseline.clone();
        changed.ops.push(OpIR {
            kind: molt_ir::OpKind::RetVoid,
            ..OpIR::new(molt_ir::OpKind::RetVoid)
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
                content_hash_for_function(
                    &changed,
                    &molt_ir::FunctionTable::default(),
                    TirPipelineCacheFlavor::Native,
                    &native
                ),
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
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "prepared_cache_identity".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: molt_ir::OpKind::RetVoid,
                ..OpIR::new(molt_ir::OpKind::RetVoid)
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: crate::ExecutionContextPolicy::None,
        };
        let options = |cache_dir: &std::path::Path| TirPipelineRunOptions {
            source_contracts: None,
            target_info: target_info.clone(),
            cache_flavor: TirPipelineCacheFlavor::FactGraph,
            cache: TirCacheLocation::Directory(cache_dir.to_path_buf()),
            process_externs: false,
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
            let functions = vec![base.clone()];
            run_cached_tir_pipeline(&functions, options(&cache_dir), |function| {
                preprocessing_calls.fetch_add(1, Ordering::Relaxed);
                function.to_mut().source_file = Some(source_file.to_string());
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
        for (tir_dump, tir_stats) in [(true, false), (false, true)] {
            let mut diagnostic_options = options(&cache_dir);
            diagnostic_options.tir_dump = tir_dump;
            diagnostic_options.tir_stats = tir_stats;
            let functions = vec![base.clone()];
            let diagnostic_run =
                run_cached_tir_pipeline(&functions, diagnostic_options, |function| {
                    function.to_mut().source_file = Some("second.py".to_string())
                });
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
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "policy_separation".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: molt_ir::OpKind::RetVoid,
                ..OpIR::new(molt_ir::OpKind::RetVoid)
            }],
            param_types: None,
            source_file: Some("policy.py".to_string()),
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: crate::ExecutionContextPolicy::None,
        };
        let hash = |policy| {
            let mut function = base.clone();
            function.execution_context = policy;
            content_hash_for_function(
                &function,
                &molt_ir::FunctionTable::default(),
                TirPipelineCacheFlavor::FactGraph,
                &target_info,
            )
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
            source_contracts: None,
            target_info: target_info.clone(),
            cache_flavor: TirPipelineCacheFlavor::FactGraph,
            cache: TirCacheLocation::Directory(cache_dir.to_path_buf()),
            process_externs: false,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: TirOptimizationResourcePlan {
                threads: 1,
                wave_function_limit: 1,
                wave_op_budget: 16,
            },
        };
        let wrong_policy = vec![base.clone()];
        let seeded = run_cached_tir_pipeline(&wrong_policy, options(&cache_dir), |_| {});
        assert_eq!(seeded.uncached_count, 1);

        let requested = vec![FunctionIR {
            execution_context: crate::ExecutionContextPolicy::Inherited,
            ..base.clone()
        }];
        let miss = run_cached_tir_pipeline(&requested, options(&cache_dir), |_| {});
        assert_eq!(
            miss.uncached_count, 1,
            "a cache artifact preseeded under the wrong execution policy must not hit"
        );
        let hit = run_cached_tir_pipeline(&requested, options(&cache_dir), |_| {});
        assert_eq!(hit.uncached_count, 0);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn prepared_body_contract_survives_relocation_and_is_replaced_on_rewrite() {
        use molt_ir::{FunctionId, FunctionTable, OpKind};

        let source = FunctionIR {
            name: "app".into(),
            ops: vec![OpIR {
                callee: Some(FunctionId::from_index(1)),
                ..OpIR::new(OpKind::CallInternal)
            }],
            ..FunctionIR::default()
        };
        let callee = FunctionIR {
            name: "provider".into(),
            ..FunctionIR::default()
        };
        let contracts_table = FunctionTable::of(&[source.clone(), callee.clone()]);
        let contract = FunctionIrContractDigest::of(&source, &contracts_table).unwrap();
        let mut relocated = source;
        relocated.ops[0].callee = Some(FunctionId::from_index(0));
        let table = FunctionTable::of(&[callee, relocated.clone()]);
        let prefix = preparation_key_prefix(TirPipelineCacheFlavor::Llvm, "test-target");
        let original_key = function_content_hash(&relocated, &table, &prefix);
        let borrowed =
            PreparedTirFunction::new(&relocated, &table, &prefix, Some(contract), &|_| {});
        assert_eq!(borrowed.content_hash, original_key);

        let rewritten =
            PreparedTirFunction::new(&relocated, &table, &prefix, Some(contract), &|row| {
                row.to_mut().frame_locals_exposed = true;
            });
        assert_ne!(rewritten.content_hash, original_key);
        assert_eq!(
            rewritten.content_hash,
            function_content_hash(&rewritten.function, &table, &prefix)
        );
    }

    #[test]
    fn cached_tir_custody_preserves_function_source_file() {
        let functions = vec![FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: molt_ir::OpKind::RetVoid,
                ..OpIR::new(molt_ir::OpKind::RetVoid)
            }],
            param_types: None,
            source_file: Some("app.py".to_string()),
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: crate::ExecutionContextPolicy::Inherited,
        }];
        let target_info = TargetInfo::native_release_fast();
        let cache_dir =
            std::env::temp_dir().join(format!("molt-tir-source-file-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let mut run = run_cached_tir_pipeline(
            &functions,
            TirPipelineRunOptions {
                source_contracts: None,
                target_info: target_info.clone(),
                cache_flavor: TirPipelineCacheFlavor::FactGraph,
                cache: TirCacheLocation::Directory(cache_dir.clone()),
                process_externs: false,
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
        let non_inlinable = HashSet::default();
        let owned_run = run_owned_module_pipeline_from_cached_tir(
            &functions,
            &mut run.cached_tir,
            TirOwnedModulePipelineOptions {
                cell_registry: None,
                module_name: "fact_graph_module",
                non_inlinable: &non_inlinable,
                missing_tir_context: "source-file test",
                mode: TirOwnedModulePipelineMode::TerminalDropsOnly,
                linkage: None,
            },
        )
        .functions;

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
