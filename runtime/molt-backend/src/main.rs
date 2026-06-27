// Windows bin-test builds compile Unix daemon protocol code for parser coverage
// without running the daemon loop; production warning policy remains unchanged.
#![cfg_attr(all(test, windows), allow(dead_code))]

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "native-backend")]
use molt_backend::SimpleBackend;
use molt_backend::SimpleIR;
#[cfg(feature = "luau-backend")]
use molt_backend::luau::LuauBackend;
#[cfg(feature = "rust-backend")]
use molt_backend::rust::RustBackend;
#[cfg(feature = "wasm-backend")]
use molt_backend::wasm::{WasmBackend, WasmCompileOptions};
use molt_tir::ir_rewrites::rewrite_annotate_stubs;
#[cfg(any(unix, test))]
use serde_json::Value as JsonValue;
#[cfg(feature = "native-backend")]
use sha2::{Digest, Sha256};
#[cfg(any(unix, test))]
use std::cmp::Reverse;
#[cfg(any(unix, test))]
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::File;
#[cfg(unix)]
use std::io::BufRead;
use std::io::Write;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(all(feature = "native-backend", windows))]
use std::os::windows::io::AsRawHandle;
use std::path::Path;
#[cfg(feature = "native-backend")]
use std::path::PathBuf;
#[cfg(any(unix, test))]
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, UnlockFileEx};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::System::IO::OVERLAPPED;

mod backend_process;
mod fact_graph_emit;
use backend_process::*;
use fact_graph_emit::{FactGraphEmitRequest, emit_fact_graph_for_ir};

#[cfg(any(unix, test))]
use molt_backend::json_boundary::{
    expect_object, optional_bool, optional_string, optional_u32, required_field, required_string,
};

#[cfg(any(unix, test))]
const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;
const DEFAULT_BACKEND_BATCH_SIZE: usize = 64;
const DEFAULT_STDLIB_BATCH_SIZE: usize = 128;
const DEFAULT_BACKEND_BATCH_OP_BUDGET: usize = 8_000;
const MIB: usize = 1024 * 1024;
const DEFAULT_DAEMON_REQUEST_LIMIT_BYTES: usize = 512 * MIB;
const DEFAULT_STDIN_REQUEST_LIMIT_BYTES: usize = DEFAULT_DAEMON_REQUEST_LIMIT_BYTES;
#[cfg(any(unix, test))]
const DEFAULT_DAEMON_MAX_JOBS: usize = 512;
#[cfg(any(unix, test))]
const DAEMON_REQUEST_ENV_KEYS: &[&str] = &[
    "MOLT_DISABLE_DEAD_FUNC_ELIM",
    "MOLT_BACKEND_BATCH_SIZE",
    "MOLT_BACKEND_BATCH_OP_BUDGET",
    "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEM_AVAILABLE_GB",
    "MOLT_MEMORY_AVAILABLE_GB",
    "MOLT_MEM_AVAILABLE_GB",
    "MOLT_BACKEND_MAX_RSS_GB",
    "MOLT_BACKEND_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEM_RESERVE_GB",
    "MOLT_MEMORY_RESERVE_GB",
    "MOLT_MEM_RESERVE_GB",
    "MOLT_MAX_FUNCTION_OPS",
    "MOLT_DISABLE_RC_COALESCING",
    "RAYON_NUM_THREADS",
    "TIR_DUMP",
    "TIR_OPT_STATS",
    "MOLT_TIR_TRACE_FUNC",
    "MOLT_DUMP_CLIF",
    "MOLT_DUMP_CLIF_ON_ERROR",
    "MOLT_DUMP_CLIF_ON_CFG_ERROR",
    "MOLT_DUMP_CLIF_FUNC",
    "MOLT_DUMP_CLIF_FILE",
    "MOLT_DUMP_CLIF_FILE_FILTER",
    "MOLT_DUMP_FINAL_FUNC_IR",
    "MOLT_DUMP_IR",
    // Optimization-pass instruments. Every optimization
    // lands WITH a firing/refusal instrument (the L4/needs_inlining lesson);
    // those instruments are useless if the daemon strips their env keys.
    // Debug-artifact routing: without these the daemon writes artifacts
    // (TIR dumps, llvm/before_opt.ll, pass refusal reports) under its own
    // CWD where nobody finds them.
    "MOLT_DEBUG_ARTIFACT_DIR",
    "MOLT_EXT_ROOT",
    "MOLT_OVERFLOW_PEEL_STATS",
    "MOLT_PROMOTE_DEBUG",
    "MOLT_INLINE_STATS",
    "MOLT_VERIFY_ANALYSIS",
    "MOLT_DEBUG_BIND",
    "MOLT_BACKEND",
    "MOLT_DEBUG_CHECK_EXC",
    "MOLT_DEBUG_CHECK_EXCEPTION",
    "MOLT_LLVM_DUMP_IR",
    "MOLT_BACKEND_TIMING",
    "MOLT_ENTRY_MODULE",
    "MOLT_STDLIB_OBJ",
    "MOLT_STDLIB_CACHE_KEY",
    "MOLT_STDLIB_CACHE_MANIFEST",
    "MOLT_STDLIB_MODULE_SYMBOLS",
    "MOLT_RUNTIME_INTRINSIC_SYMBOLS",
    "MOLT_DEBUG_DROP",
    "MOLT_DEBUG_LOWER_FUNC",
    "MOLT_TIR_DUMP",
];

#[cfg(feature = "rust-backend")]
fn rust_source_for_ir(ir: &SimpleIR) -> io::Result<String> {
    let mut ir = ir.clone();
    ir.tree_shake_source_emission();
    let mut backend = RustBackend::new();
    backend.compile_checked(&ir).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Rust validation failed: {err}"),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LuauTirModulePipelineStats {
    functions: usize,
    module_changed: usize,
}

fn run_luau_tir_module_pipeline(ir: &mut SimpleIR) -> io::Result<LuauTirModulePipelineStats> {
    let target_info = molt_backend::tir::target_info::TargetInfo::luau_release_fast();
    let (mut tir_module, idx_map) =
        molt_backend::tir::lower_from_simple::lower_functions_to_tir_module(&ir.functions);
    tir_module.name = "luau_module".to_string();

    for tir_func in &mut tir_module.functions {
        molt_backend::tir::type_refine::refine_types(tir_func);
        let _stats = molt_backend::tir::passes::run_pipeline(tir_func, &target_info);
        molt_backend::tir::type_refine::refine_types(tir_func);
    }

    let non_inlinable = std::collections::HashSet::new();
    let module_analysis =
        molt_backend::tir::run_module_pipeline(&mut tir_module, &target_info, &non_inlinable);

    for (pos, &orig_idx) in idx_map.iter().enumerate() {
        let tir_func = &tir_module.functions[pos];
        let ops = molt_backend::tir::lower_to_simple::lower_to_simple_ir(tir_func);
        if !molt_backend::tir::lower_to_simple::validate_labels(&ops) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Luau TIR module back-conversion emitted invalid labels for '{}'",
                    tir_func.name
                ),
            ));
        }
        ir.functions[orig_idx].ops = ops;
    }

    Ok(LuauTirModulePipelineStats {
        functions: tir_module.functions.len(),
        module_changed: module_analysis.changed_functions.len(),
    })
}

const GIB: u64 = 1024 * 1024 * 1024;

fn default_backend_max_rss_gb_from_physical_mem_bytes(bytes: Option<u64>) -> u64 {
    match bytes.map(|raw| raw / GIB).unwrap_or(0) {
        gib if gib >= 64 => 16,
        gib if gib >= 32 => 12,
        gib if gib >= 16 => 8,
        _ => 4,
    }
}

#[cfg(unix)]
fn detect_physical_memory_bytes() -> Option<u64> {
    unsafe {
        let pages = libc::sysconf(libc::_SC_PHYS_PAGES);
        let page_size = libc::sysconf(libc::_SC_PAGESIZE);
        if pages <= 0 || page_size <= 0 {
            return None;
        }
        Some((pages as u64).saturating_mul(page_size as u64))
    }
}

#[cfg(windows)]
fn detect_physical_memory_bytes() -> Option<u64> {
    unsafe {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = core::mem::zeroed();
        status.dwLength = core::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            return None;
        }
        Some(status.ullTotalPhys)
    }
}

#[cfg(not(any(unix, windows)))]
fn detect_physical_memory_bytes() -> Option<u64> {
    None
}

fn default_backend_max_rss_gb() -> u64 {
    default_backend_max_rss_gb_from_physical_mem_bytes(detect_physical_memory_bytes())
}

fn validate_fact_graph_cli_contract(
    output_path: Option<&str>,
    function_name: Option<&str>,
    is_rust: bool,
) -> io::Result<()> {
    if output_path.is_some() != function_name.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--fact-graph-output and --fact-graph-function must be supplied together",
        ));
    }
    if output_path.is_some() && is_rust {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fact graph emission does not support the rust target",
        ));
    }
    Ok(())
}

#[allow(clippy::vec_init_then_push)] // pushes are behind #[cfg] feature gates
fn main() -> io::Result<()> {
    // TIR optimization is mandatory. Invalid roundtrips are fatal compiler
    // bugs and must be debugged through dumps/verifier evidence, not by
    // bypassing typed IR.

    // Hard memory guard: set rlimit on virtual memory to prevent OOM
    // from crashing the entire machine. The default scales with host memory
    // so large TIR-enabled stdlib builds do not trip an artificially tiny cap.
    #[cfg(unix)]
    {
        let max_gb: u64 = std::env::var("MOLT_BACKEND_MAX_RSS_GB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_backend_max_rss_gb);
        let max_bytes = max_gb * 1024 * 1024 * 1024;
        unsafe {
            let rlim = libc::rlimit {
                rlim_cur: max_bytes,
                rlim_max: max_bytes,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                // Silently ignore on macOS (Apple Silicon). MOLT_DEBUG_RLIMIT=1 to warn.
                if std::env::var("MOLT_DEBUG_RLIMIT").as_deref() == Ok("1") {
                    eprintln!(
                        "WARNING: failed to set memory limit (RLIMIT_AS={max_gb}GB). OOM guard not active."
                    );
                }
            }
        }
    }

    // Windows memory guard: use job objects to limit working set.
    // Less effective than Unix RLIMIT_AS but prevents unbounded growth.
    #[cfg(windows)]
    {
        let max_gb: u64 = std::env::var("MOLT_BACKEND_MAX_RSS_GB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_backend_max_rss_gb);
        let max_bytes = max_gb * 1024 * 1024 * 1024;
        unsafe {
            use windows_sys::Win32::System::JobObjects::*;
            use windows_sys::Win32::System::Threading::*;
            let job = CreateJobObjectW(core::ptr::null(), core::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = core::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = max_bytes as usize;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                AssignProcessToJobObject(job, GetCurrentProcess());
            }
        }
    }

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--features") {
        let features: &[&str] = &[
            #[cfg(feature = "native-backend")]
            "native-backend",
            #[cfg(feature = "luau-backend")]
            "luau-backend",
            #[cfg(feature = "wasm-backend")]
            "wasm-backend",
            #[cfg(feature = "rust-backend")]
            "rust-backend",
            #[cfg(feature = "cbor")]
            "cbor",
        ];
        if features.is_empty() {
            println!("molt-backend: no features enabled");
        } else {
            println!("molt-backend features: {}", features.join(", "));
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--daemon") {
        let socket_path = args
            .iter()
            .position(|arg| arg == "--socket")
            .and_then(|idx| args.get(idx + 1))
            .map(String::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))?;
        return run_daemon(socket_path);
    }
    let is_wasm = args.contains(&"--target".to_string()) && args.contains(&"wasm".to_string());
    let is_rust = args.contains(&"--target".to_string()) && args.contains(&"rust".to_string());
    let is_luau = args.contains(&"--target".to_string()) && args.contains(&"luau".to_string());
    #[allow(unused_variables)]
    let use_ir_pipeline = args.contains(&"--ir-pipeline".to_string());
    #[cfg_attr(not(feature = "native-backend"), allow(unused_variables))]
    let target_triple = args
        .iter()
        .position(|arg| arg == "--target-triple")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let output_path = args
        .iter()
        .position(|arg| arg == "--output")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    #[cfg(feature = "native-backend")]
    let native_batch_job_file = args
        .iter()
        .position(|arg| arg == "--native-batch-job-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    #[cfg(feature = "native-backend")]
    if let Some(job_file) = native_batch_job_file {
        let output_file = output_path.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--output is required with --native-batch-job-file",
            )
        })?;
        compile_native_batch_object_job_file(Path::new(job_file), Path::new(output_file))?;
        return Ok(());
    }

    let ir_file_path = args
        .iter()
        .position(|arg| arg == "--ir-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let fact_graph_output_path = args
        .iter()
        .position(|arg| arg == "--fact-graph-output")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let fact_graph_function = args
        .iter()
        .position(|arg| arg == "--fact-graph-function")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    validate_fact_graph_cli_contract(fact_graph_output_path, fact_graph_function, is_rust)?;

    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_link_flag = args.iter().any(|arg| arg == "--wasm-link");
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_data_base = args
        .iter()
        .position(|arg| arg == "--wasm-data-base")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_table_base = args
        .iter()
        .position(|arg| arg == "--wasm-table-base")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_split_runtime_runtime_table_min = args
        .iter()
        .position(|arg| arg == "--wasm-split-runtime-runtime-table-min")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());

    let ir_format = args
        .iter()
        .position(|arg| arg == "--ir-format")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str)
        .unwrap_or("json");

    // Read and parse IR.  Drop the raw buffer immediately after
    // deserialization to avoid holding two copies in memory simultaneously.
    let stdin_request_limit_bytes = stdin_request_limit_bytes();
    let mut ir: SimpleIR = {
        if ir_format == "msgpack" {
            // msgpack binary format — deserialize directly via serde
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path).map_err(|e| {
                    io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                })?;
                let reader = io::BufReader::new(file);
                match rmp_serde::from_read(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                // Streaming msgpack from stdin via BufReader — avoids
                // loading the entire IR into a Vec<u8> first.
                let stdin = io::stdin();
                let bounded = RequestBoundedRead::new(
                    stdin.lock(),
                    stdin_request_limit_bytes,
                    "backend stdin request",
                );
                let reader = io::BufReader::with_capacity(1 << 20, bounded);
                match rmp_serde::from_read::<_, SimpleIR>(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            }
        } else if ir_format == "cbor" {
            // CBOR binary format — deserialize via ciborium
            #[cfg(not(feature = "cbor"))]
            {
                eprintln!("CBOR support requires the 'cbor' feature");
                std::process::exit(1);
            }
            #[cfg(feature = "cbor")]
            {
                if let Some(ir_path) = ir_file_path {
                    let file = std::fs::File::open(ir_path).map_err(|e| {
                        io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                    })?;
                    let reader = io::BufReader::new(file);
                    match ciborium::de::from_reader(reader) {
                        Ok(ir) => ir,
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    let buf = read_bounded_request_bytes(
                        io::stdin().lock(),
                        stdin_request_limit_bytes,
                        "backend stdin request",
                    )?;
                    match ciborium::de::from_reader::<SimpleIR, _>(&buf[..]) {
                        Ok(ir) => {
                            drop(buf);
                            ir
                        }
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        } else if ir_format == "ndjson" {
            // NDJSON streaming format — one function per line
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path).map_err(|e| {
                    io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                })?;
                let reader = io::BufReader::new(file);
                match SimpleIR::from_ndjson_reader(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid NDJSON IR: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                let stdin = io::stdin();
                let bounded = RequestBoundedRead::new(
                    stdin.lock(),
                    stdin_request_limit_bytes,
                    "backend stdin request",
                );
                let reader = io::BufReader::new(bounded);
                match SimpleIR::from_ndjson_reader(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid NDJSON IR: {err}");
                        std::process::exit(1);
                    }
                }
            }
        } else if let Some(ir_path) = ir_file_path {
            // Stream JSON directly from file — never holds raw JSON string in memory.
            let file = std::fs::File::open(ir_path).map_err(|e| {
                io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
            })?;
            let reader = io::BufReader::with_capacity(1 << 20, file);
            match serde_json::from_reader::<_, SimpleIR>(reader) {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        } else {
            // Stdin: read into string then deserialize directly (skips DOM intermediate).
            let raw_bytes = read_bounded_request_bytes(
                io::stdin().lock(),
                stdin_request_limit_bytes,
                "backend stdin request",
            )?;
            let buffer = String::from_utf8(raw_bytes).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("backend stdin request is not UTF-8: {err}"),
                )
            })?;
            let result = serde_json::from_str::<SimpleIR>(&buffer);
            drop(buffer);
            match result {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        }
    };

    rewrite_annotate_stubs(&mut ir);

    // Source emitters do not link against native/WASM runtime objects, so they
    // prune unreachable runtime/bootstrap support before textual codegen.
    if is_luau {
        ir.tree_shake_luau();
    }

    if let (Some(path), Some(function_name)) = (fact_graph_output_path, fact_graph_function) {
        let backend_setting = std::env::var("MOLT_BACKEND").ok();
        let target_info = if is_luau {
            molt_backend::tir::target_info::TargetInfo::luau_release_fast()
        } else if is_wasm {
            molt_backend::tir::target_info::TargetInfo::wasm_release_fast()
        } else if backend_setting.as_deref() == Some("llvm") {
            molt_backend::tir::target_info::TargetInfo::llvm_release_fast()
        } else {
            molt_backend::tir::target_info::TargetInfo::native_release_fast()
        };
        emit_fact_graph_for_ir(
            &ir,
            FactGraphEmitRequest {
                output_path: Path::new(path),
                function_name,
                target_info: &target_info,
            },
        )?;
        eprintln!("Wrote TIR fact graph for '{function_name}' to {path}");
        return Ok(());
    }

    // Luau module phase (Tier-2 E1 parity): source emission is still one
    // compilation unit, so every local body is owned by this module and the
    // inliner has no external-linkage exclusions. Keep Luau on the same
    // structural path as native/WASM: lift once to TIR, run every local
    // function through the per-function pipeline, then run the whole-module
    // pipeline (E1 inliner, generator fusion, module-slot promotion, terminal
    // DropInsertion) before one fail-closed back-conversion.
    if is_luau {
        let tir_start = Instant::now();
        let module_stats = run_luau_tir_module_pipeline(&mut ir)?;
        let tir_elapsed = tir_start.elapsed();
        eprintln!(
            "[molt-luau] TIR module pipeline: {} functions, {} module-changed in {tir_elapsed:.2?}",
            module_stats.functions, module_stats.module_changed
        );
        molt_backend::eliminate_dead_ops(&mut ir);
    }

    let output_kind = if is_luau {
        BackendOutputKind::Luau
    } else if is_rust {
        BackendOutputKind::Rust
    } else if is_wasm {
        BackendOutputKind::Wasm
    } else {
        BackendOutputKind::Native
    };
    let output_file = resolve_backend_output_path(output_path, output_kind);
    ensure_output_parent_dir(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to create backend output parent for '{}': {}",
                output_file, err
            ),
        )
    })?;
    if is_luau {
        #[cfg(feature = "luau-backend")]
        {
            let mut backend = LuauBackend::new();
            let source = if use_ir_pipeline {
                backend.compile_via_ir(&ir)
            } else {
                backend.compile_checked(&ir)
            }
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Luau validation failed for '{}': {}", output_file, err),
                )
            })?;
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            file.write_all(source.as_bytes())?;
            let lines = source.lines().count();
            eprintln!(
                "Successfully transpiled to {output_file} ({lines} lines, {:.1} KB)",
                source.len() as f64 / 1024.0
            );
        }
        #[cfg(not(feature = "luau-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without luau-backend support; rebuild with: cargo build -p molt-backend --features luau-backend",
            ));
        }
    } else if is_rust {
        #[cfg(feature = "rust-backend")]
        {
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            let source = rust_source_for_ir(&ir)?;
            file.write_all(source.as_bytes())?;
            println!("Successfully transpiled to {output_file}");
        }
        #[cfg(not(feature = "rust-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without rust-backend support; rebuild with: cargo build -p molt-backend --features rust-backend",
            ));
        }
    } else if is_wasm {
        #[cfg(feature = "wasm-backend")]
        {
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            let mut options = WasmCompileOptions::default();
            if wasm_link_flag {
                options.reloc_enabled = true;
            }
            if let Some(value) = wasm_data_base {
                options.data_base = value;
            }
            if let Some(value) = wasm_table_base {
                options.table_base = value;
            }
            if let Some(value) = wasm_split_runtime_runtime_table_min {
                options.split_runtime_runtime_table_min = Some(value);
            }
            let backend = WasmBackend::with_options(options);
            let wasm_bytes = backend.compile(ir);
            file.write_all(&wasm_bytes)?;
            println!("Successfully compiled to {output_file}");
        }
        #[cfg(not(feature = "wasm-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend",
            ));
        }
    } else {
        #[cfg(feature = "native-backend")]
        {
            // ── Incremental compilation ──
            // When MOLT_STDLIB_OBJ is set to a path, the backend caches
            // stdlib compilation: stdlib functions compile once to that path,
            // subsequent builds skip them entirely.  User functions always
            // recompile.  This reduces builds from ~5min to ~3sec.
            let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
            let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
            let expected_stdlib_cache_manifest = std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
            let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
            let entry_module =
                std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
            let explicit_stdlib_module_symbols = molt_backend::stdlib_module_symbols_from_env()
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
            let compile_options = prepare_native_application_object(
                &mut ir,
                NativeStdlibCachePrepare {
                    target_triple,
                    stdlib_obj_path: stdlib_obj_path.as_deref(),
                    expected_cache_key: expected_stdlib_cache_key.as_deref(),
                    expected_cache_manifest: expected_stdlib_cache_manifest.as_deref(),
                    have_entry_module,
                    entry_module: &entry_module,
                    explicit_stdlib_module_symbols: explicit_stdlib_module_symbols.as_ref(),
                    log_prefix: "MOLT_BACKEND",
                },
            )?;

            compile_native_application_object_to_path(ir, Path::new(output_file), compile_options)?;
        }
        #[cfg(not(feature = "native-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend",
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "rust-backend")]
    use super::rust_source_for_ir;
    use super::{
        BACKEND_DAEMON_PROTOCOL_VERSION, BackendOutputKind, DEFAULT_BACKEND_BATCH_OP_BUDGET,
        DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE, DaemonCache, DaemonJobRequest,
        DaemonRequest, GIB, MIB, NativeApplicationObjectOptions, RequestBoundedRead,
        compile_native_application_object_to_path, compile_single_job, compile_stdlib_cache_object,
        create_backend_output_file, default_backend_max_rss_gb_from_physical_mem_bytes,
        default_backend_output_path, default_daemon_cache_bytes_from_physical_mem_bytes,
        ensure_output_parent_dir, is_user_owned_symbol, merge_relocatable_objects,
        partition_functions_for_batches, preserve_native_batch_worker_failure_artifacts,
        prune_and_partition_native_stdlib, read_bounded_request_bytes, read_json_artifact,
        read_stdlib_cache_key, read_stdlib_cache_manifest, relocatable_linker_binary,
        remove_native_batch_temp_dir, resolve_backend_output_path, resolved_batch_op_budget_limit,
        resolved_batch_size_limit, run_luau_tir_module_pipeline, shared_stdlib_cache_matches,
        shared_stdlib_partition_closure_issue, shared_stdlib_partition_manifest,
        stdlib_cache_count_sidecar_path, stdlib_cache_partition_manifest_sidecar_path,
        validate_fact_graph_cli_contract, validate_shared_stdlib_partition,
        with_shared_stdlib_cache_publish_lock, write_cached_output, write_json_artifact,
        write_shared_stdlib_cache_sidecars,
    };
    #[cfg(unix)]
    use super::{DaemonResponse, daemon_response_payload, read_daemon_request_bytes};
    use super::{NativeBatchModuleMetadata, NativeBatchObjectJob};
    use molt_backend::{FunctionIR, OpIR, SimpleIR};
    use std::io::{self, Cursor, Read, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());
    const SHARED_STDLIB_CACHE_ENV_KEYS: &[&str] = &[
        "MOLT_STDLIB_OBJ",
        "MOLT_STDLIB_CACHE_KEY",
        "MOLT_STDLIB_CACHE_MANIFEST",
    ];

    #[test]
    fn fact_graph_cli_contract_requires_output_and_function_pair() {
        let err = validate_fact_graph_cli_contract(Some("graph.json"), None, false)
            .expect_err("unpaired fact graph flags must fail closed");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string()
                .contains("--fact-graph-output and --fact-graph-function")
        );
    }

    #[test]
    fn fact_graph_cli_contract_rejects_rust_target() {
        let err = validate_fact_graph_cli_contract(Some("graph.json"), Some("molt_main"), true)
            .expect_err("rust target fact graph request must fail closed");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("rust target"));
    }

    struct TestEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        snapshot: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnvGuard {
        fn capture(keys: &'static [&'static str]) -> Self {
            let lock = ENV_TEST_MUTEX
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            Self {
                _lock: lock,
                snapshot,
            }
        }

        fn clear(keys: &'static [&'static str]) -> Self {
            let guard = Self::capture(keys);
            for (key, _) in &guard.snapshot {
                unsafe { std::env::remove_var(key) };
            }
            guard
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.snapshot {
                match value {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    fn write_failing_relocatable_linker(tmp_dir: &std::path::Path) -> std::path::PathBuf {
        #[cfg(windows)]
        let linker = tmp_dir.join("fail-linker.cmd");
        #[cfg(not(windows))]
        let linker = tmp_dir.join("fail-linker.sh");

        #[cfg(windows)]
        std::fs::write(
            &linker,
            b"@echo off\r\necho forced relocatable link failure 1>&2\r\nexit /b 1\r\n",
        )
        .expect("write failing linker script");

        #[cfg(not(windows))]
        {
            std::fs::write(
                &linker,
                b"#!/bin/sh\necho forced relocatable link failure >&2\nexit 1\n",
            )
            .expect("write failing linker script");
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&linker)
                .expect("stat failing linker script")
                .permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&linker, permissions)
                .expect("make failing linker script executable");
        }

        linker
    }

    #[test]
    fn daemon_cache_get_bytes_updates_lru_without_cloning() {
        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1, 2, 3, 4]));

        let bytes = cache.get_bytes("module").expect("cache hit");
        assert_eq!(bytes, &[1, 2, 3, 4]);

        let entry = cache.entries.get("module").expect("entry retained");
        assert_eq!(entry.bytes.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(entry.stamp, cache.clock);
    }

    #[test]
    fn daemon_cache_can_share_bytes_across_keys() {
        let mut cache = DaemonCache::new(None);
        let shared = Arc::<[u8]>::from(vec![9, 8, 7, 6]);
        cache.insert("module".to_string(), Arc::clone(&shared));
        cache.insert("function".to_string(), shared);

        let module = cache.entries.get("module").expect("module entry");
        let function = cache.entries.get("function").expect("function entry");
        assert!(Arc::ptr_eq(&module.bytes, &function.bytes));
    }

    #[test]
    fn luau_tir_module_pipeline_inlines_direct_local_calls() {
        let callee = FunctionIR {
            name: "luau_add1".to_string(),
            params: vec!["x".to_string()],
            param_types: Some(vec!["int".to_string()]),
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("one".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["x".to_string(), "one".to_string()]),
                    out: Some("sum".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["sum".to_string()]),
                    ..OpIR::default()
                },
            ],
            source_file: None,
            is_extern: false,
        };
        let caller = FunctionIR {
            name: "molt_main".to_string(),
            params: Vec::new(),
            param_types: None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(41),
                    out: Some("arg".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("luau_add1".to_string()),
                    args: Some(vec!["arg".to_string()]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            source_file: None,
            is_extern: false,
        };
        let mut ir = SimpleIR {
            functions: vec![caller, callee],
            profile: None,
        };

        let stats = run_luau_tir_module_pipeline(&mut ir).expect("luau module pipeline");

        assert_eq!(stats.functions, 2);
        assert!(
            stats.module_changed >= 1,
            "direct call inlining must report at least one changed function"
        );
        let main = ir
            .functions
            .iter()
            .find(|func| func.name == "molt_main")
            .expect("molt_main");
        assert!(
            main.ops
                .iter()
                .all(|op| !(op.kind == "call" && op.s_value.as_deref() == Some("luau_add1"))),
            "Luau module phase must inline direct local calls instead of leaving a call boundary: {:?}",
            main.ops
        );
    }

    #[test]
    fn daemon_native_path_written_output_skips_oversized_memory_cache() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tracked_env = [
            "MOLT_BACKEND_BATCH_SIZE",
            "MOLT_LINKER",
            "MOLT_STDLIB_OBJ",
            "MOLT_STDLIB_CACHE_KEY",
            "MOLT_STDLIB_CACHE_MANIFEST",
            "MOLT_STDLIB_MODULE_SYMBOLS",
            "MOLT_RUNTIME_INTRINSIC_SYMBOLS",
            "MOLT_ENTRY_MODULE",
        ];
        let prior_env: Vec<_> = tracked_env
            .iter()
            .map(|name| (*name, std::env::var(name).ok()))
            .collect();
        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1");
            std::env::set_var("MOLT_LINKER", "ld");
            std::env::remove_var("MOLT_STDLIB_OBJ");
            std::env::remove_var("MOLT_STDLIB_CACHE_KEY");
            std::env::remove_var("MOLT_STDLIB_CACHE_MANIFEST");
            std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS");
            std::env::remove_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS");
            std::env::remove_var("MOLT_ENTRY_MODULE");
        }

        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-native-cache-budget-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let job = DaemonJobRequest {
            id: "job0".to_string(),
            is_wasm: false,
            target_triple: None,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_runtime_table_min: None,
            output: output.to_string_lossy().to_string(),
            cache_key: "module-cache".to_string(),
            function_cache_key: Some("function-cache".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: false,
            ir: Some(SimpleIR {
                functions: vec![
                    FunctionIR {
                        name: "molt_main".to_string(),
                        params: vec![],
                        ops: vec![
                            OpIR {
                                kind: "call".to_string(),
                                s_value: Some("helper".to_string()),
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
                        name: "helper".to_string(),
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
            }),
            ir_path: None,
        };
        let mut cache = DaemonCache::new(Some(1));

        let result = compile_single_job(job, &mut cache);

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "path-written daemon output missing");
        assert!(
            cache.entries.is_empty(),
            "oversized object must not enter daemon memory cache"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("skipped daemon memory cache")),
            "missing cache-budget warning: {:?}",
            result.warnings
        );

        for (name, value) in prior_env {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn read_daemon_request_bytes_stops_at_protocol_newline() {
        let mut cursor = Cursor::new(b"{\"version\":1}\ntrailing".to_vec());
        let bytes = read_daemon_request_bytes(&mut cursor, 1024).expect("request bytes");
        assert_eq!(bytes, b"{\"version\":1}\n");
    }

    #[cfg(unix)]
    #[test]
    fn read_daemon_request_bytes_rejects_oversized_request() {
        let mut cursor = Cursor::new(b"{\"version\":1}\n".to_vec());
        let err = read_daemon_request_bytes(&mut cursor, 4).expect_err("oversized request");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("daemon request exceeded 4 byte limit")
        );
    }

    #[test]
    fn read_bounded_request_bytes_allows_exact_limit() {
        let cursor = Cursor::new(b"abcd".to_vec());
        let bytes =
            read_bounded_request_bytes(cursor, 4, "backend stdin request").expect("request bytes");
        assert_eq!(bytes, b"abcd");
    }

    #[test]
    fn read_bounded_request_bytes_rejects_oversized_request() {
        let cursor = Cursor::new(b"abcde".to_vec());
        let err = read_bounded_request_bytes(cursor, 4, "backend stdin request")
            .expect_err("oversized request");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("backend stdin request exceeded 4 byte limit")
        );
    }

    #[test]
    fn request_bounded_read_rejects_streaming_read_past_limit() {
        let cursor = Cursor::new(b"abcde".to_vec());
        let mut reader = RequestBoundedRead::new(cursor, 4, "streaming backend stdin request");
        let mut first_chunk = [0_u8; 4];
        assert_eq!(
            reader.read(&mut first_chunk).expect("first bounded read"),
            4
        );
        assert_eq!(&first_chunk, b"abcd");
        let mut probe = [0_u8; 1];
        let err = reader.read(&mut probe).expect_err("stream overflow");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .contains("streaming backend stdin request exceeded 4 byte limit")
        );
    }

    #[test]
    fn daemon_default_cache_limit_scales_with_host_memory() {
        assert_eq!(
            default_daemon_cache_bytes_from_physical_mem_bytes(Some(8 * GIB)),
            128 * MIB
        );
        assert_eq!(
            default_daemon_cache_bytes_from_physical_mem_bytes(Some(128 * GIB)),
            2 * 1024 * MIB
        );
    }

    #[test]
    fn daemon_request_parse_applies_boolean_defaults() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": false,
                        "output": "/tmp/out.o",
                        "cache_key": "module"
                    }
                ]
            }"#,
        )
        .expect("request parse");

        let job = request.jobs.expect("job list").pop().expect("job");
        assert!(!job.wasm_link);
        assert_eq!(job.wasm_split_runtime_runtime_table_min, None);
        assert!(!job.skip_module_output_if_synced);
        assert!(!job.skip_function_output_if_synced);
        assert!(!job.probe_cache_only);
        assert!(job.ir.is_none());
        assert!(job.ir_path.is_none());
    }

    #[test]
    fn daemon_request_parse_accepts_path_backed_ir_lease() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": false,
                        "output": "/tmp/out.o",
                        "cache_key": "module",
                        "ir_path": "/tmp/molt-ir.json"
                    }
                ]
            }"#,
        )
        .expect("request parse");

        let job = request.jobs.expect("job list").pop().expect("job");
        assert!(job.ir.is_none());
        assert_eq!(job.ir_path.as_deref(), Some("/tmp/molt-ir.json"));
    }

    #[test]
    fn daemon_request_parse_rejects_duplicate_ir_authority() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let err = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": false,
                        "output": "/tmp/out.o",
                        "cache_key": "module",
                        "ir_path": "/tmp/molt-ir.json",
                        "ir": {"functions": []}
                    }
                ]
            }"#,
        )
        .expect_err("duplicate IR sources");

        assert!(
            err.contains("request.jobs[0] must use exactly one IR custody field: ir or ir_path")
        );
    }

    #[test]
    fn daemon_request_parse_reads_split_runtime_table_min() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = DaemonRequest::from_json_bytes(
            br#"{
                "version": 1,
                "jobs": [
                    {
                        "id": "job0",
                        "is_wasm": true,
                        "wasm_link": true,
                        "wasm_data_base": 1048576,
                        "wasm_table_base": 4096,
                        "wasm_split_runtime_runtime_table_min": 8192,
                        "output": "/tmp/out.wasm",
                        "cache_key": "module"
                    }
                ]
            }"#,
        )
        .expect("request parse");

        let job = request.jobs.expect("job list").pop().expect("job");
        assert!(job.wasm_link);
        assert_eq!(job.wasm_data_base, Some(1048576));
        assert_eq!(job.wasm_table_base, Some(4096));
        assert_eq!(job.wasm_split_runtime_runtime_table_min, Some(8192));
    }

    #[cfg(unix)]
    #[test]
    fn daemon_response_payload_omits_false_optional_fields() {
        let payload = daemon_response_payload(&DaemonResponse {
            ok: true,
            pong: false,
            jobs: vec![super::DaemonJobResponse {
                id: "job0".to_string(),
                ok: true,
                cached: false,
                cache_tier: None,
                output_written: true,
                needs_ir: false,
                message: None,
                warnings: Vec::new(),
            }],
            error: None,
            health: None,
        })
        .expect("response payload");

        let text = String::from_utf8(payload).expect("utf8 json");
        assert!(!text.contains("\"needs_ir\""));
        assert!(!text.contains("\"health\""));
        assert!(!text.contains("\"error\""));
    }

    #[test]
    fn write_cached_output_can_skip_disk_write_when_synced() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!("molt-backend-test-{nonce}.o"));

        let written = write_cached_output(output.to_str().expect("utf8 path"), b"artifact", true)
            .expect("cache hit succeeds");

        assert!(!written);
        assert!(!output.exists());
    }

    #[test]
    fn ensure_output_parent_dir_creates_nested_directories() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("molt-backend-parent-{nonce}"));
        let output = root.join("nested").join("cache").join("artifact.wasm");

        ensure_output_parent_dir(output.to_str().expect("utf8 path")).expect("parent dir creation");

        assert!(output.parent().expect("parent exists").is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_backend_output_file_recreates_missing_parent() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("molt-backend-create-{nonce}"));
        let output = root.join("nested").join("cache").join("artifact.o");

        ensure_output_parent_dir(output.to_str().expect("utf8 path")).expect("prime parent");
        std::fs::remove_dir_all(root.join("nested")).expect("remove parent tree");

        let mut file =
            create_backend_output_file(output.to_str().expect("utf8 path")).expect("create file");
        file.write_all(b"artifact").expect("write artifact");
        drop(file);

        assert_eq!(std::fs::read(&output).expect("read artifact"), b"artifact");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn native_batch_temp_cleanup_reports_non_directory_path() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("molt-backend-cleanup-file-{nonce}"));
        std::fs::write(&path, b"not-a-directory").expect("write cleanup sentinel");

        let err = remove_native_batch_temp_dir(&path, "native batch cleanup test")
            .expect_err("file path must not be silently accepted as cleaned temp dir");

        assert!(
            err.to_string()
                .contains("failed to remove native batch cleanup test"),
            "unexpected cleanup error: {err}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn native_batch_failure_artifact_rewrites_context_path_for_replay() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_debug_artifact_dir = std::env::var("MOLT_DEBUG_ARTIFACT_DIR").ok();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "molt-batch-failure-artifact-test-{}-{nonce}",
            std::process::id()
        ));
        let source_dir = root.join("source");
        let debug_dir = root.join("debug");
        std::fs::create_dir_all(&source_dir).expect("create source batch dir");
        unsafe { std::env::set_var("MOLT_DEBUG_ARTIFACT_DIR", &debug_dir) };

        let module_context_path = source_dir.join("module_context.json");
        write_json_artifact(
            &module_context_path,
            &NativeBatchModuleMetadata {
                module_context: molt_backend::NativeBackendModuleContext::default(),
            },
        )
        .expect("write source module context");
        let job_path = source_dir.join("batch_7.json");
        write_json_artifact(
            &job_path,
            &NativeBatchObjectJob {
                ir: SimpleIR {
                    functions: vec![],
                    profile: None,
                },
                module_context_path: module_context_path.clone(),
                target_triple: None,
                emit_app_intrinsic_resolver: false,
                app_intrinsic_manifest: None,
                external_function_names: std::collections::BTreeSet::new(),
            },
        )
        .expect("write source job");
        let object_path = source_dir.join("batch_7.o");

        let artifact_dir = preserve_native_batch_worker_failure_artifacts(
            "native application batch worker",
            &job_path,
            &object_path,
        )
        .expect("preserve failed worker artifacts");
        std::fs::remove_dir_all(&source_dir).expect("source batch temp dir cleanup");

        let copied_job_path = artifact_dir.join("batch_7.json");
        let copied_job: NativeBatchObjectJob =
            read_json_artifact(&copied_job_path, "copied native batch job")
                .expect("read copied native batch job");
        assert_eq!(
            copied_job.module_context_path,
            artifact_dir.join("module_context.json")
        );
        assert!(
            copied_job.module_context_path.exists(),
            "copied module context must survive source cleanup"
        );
        assert!(
            artifact_dir.join("manifest.json").exists(),
            "artifact manifest must describe replay command"
        );

        match prior_debug_artifact_dir {
            Some(value) => unsafe { std::env::set_var("MOLT_DEBUG_ARTIFACT_DIR", value) },
            None => unsafe { std::env::remove_var("MOLT_DEBUG_ARTIFACT_DIR") },
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_backend_output_paths_use_dist_root() {
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Luau),
            "dist/output.luau"
        );
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Rust),
            "dist/output.rs"
        );
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Wasm),
            "dist/output.wasm"
        );
        assert_eq!(
            default_backend_output_path(BackendOutputKind::Native),
            "dist/output.o"
        );
    }

    #[test]
    fn resolve_backend_output_path_prefers_explicit_output() {
        let explicit = "/tmp/custom/output.wasm";
        assert_eq!(
            resolve_backend_output_path(Some(explicit), BackendOutputKind::Wasm),
            explicit
        );
        assert_eq!(
            resolve_backend_output_path(None, BackendOutputKind::Wasm),
            "dist/output.wasm"
        );
    }

    #[cfg(feature = "rust-backend")]
    #[test]
    fn rust_source_for_ir_rejects_stub_markers() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "unsupported_for_rust_target_test".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let err = rust_source_for_ir(&ir).expect_err("Rust target must reject stub markers");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("unimplemented op stubs"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "rust-backend")]
    #[test]
    fn rust_source_for_ir_prunes_unreachable_stub_markers() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "dead_stdlib_helper".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "unsupported_for_rust_target_test".to_string(),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let source = rust_source_for_ir(&ir).expect("dead stubs must be pruned before Rust emit");
        assert!(source.contains("fn molt_main("));
        assert!(!source.contains("dead_stdlib_helper"));
        assert!(!source.contains("MOLT_STUB"));
    }

    #[test]
    fn daemon_probe_cache_only_returns_needs_ir_on_miss() {
        let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                wasm_split_runtime_runtime_table_min: None,
                output: "/tmp/unused.o".to_string(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
                ir_path: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(!result.cached);
        assert!(result.needs_ir);
        assert!(!result.output_written);
    }

    #[test]
    fn daemon_probe_cache_only_hits_without_ir() {
        let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1_u8, 2, 3]));
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!("molt-backend-probe-hit-{nonce}.o"));

        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                wasm_split_runtime_runtime_table_min: None,
                output: output.to_string_lossy().into_owned(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
                ir_path: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(result.cached);
        assert!(!result.needs_ir);
        assert!(output.exists());
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn daemon_cache_hit_requires_matching_shared_stdlib_artifact() {
        let _env_guard = TestEnvGuard::capture(SHARED_STDLIB_CACHE_ENV_KEYS);

        let mut cache = DaemonCache::new(None);
        cache.insert("module".to_string(), Arc::from(vec![1_u8, 2, 3]));
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("molt-backend-stdlib-cache-{nonce}"));
        let output = root.join("probe-hit.o");
        let missing_stdlib = root.join("missing-stdlib.o");
        std::fs::create_dir_all(&root).expect("create temp dir");
        unsafe {
            std::env::set_var("MOLT_STDLIB_OBJ", &missing_stdlib);
            std::env::set_var("MOLT_STDLIB_CACHE_KEY", "stdlib-key");
            std::env::set_var("MOLT_STDLIB_CACHE_MANIFEST", "stdlib-manifest");
        }

        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                is_wasm: false,
                target_triple: None,
                wasm_link: false,
                wasm_data_base: None,
                wasm_table_base: None,
                wasm_split_runtime_runtime_table_min: None,
                output: output.to_string_lossy().into_owned(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
                ir_path: None,
            },
            &mut cache,
        );

        assert!(result.ok);
        assert!(!result.cached);
        assert!(result.needs_ir);
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn user_owned_symbol_partition_uses_explicit_stdlib_modules() {
        let stdlib_modules =
            std::collections::BTreeSet::from(["sys".to_string(), "json".to_string()]);

        assert!(is_user_owned_symbol(
            "molt_main",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_host_init",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "app__module",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_init_app",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_init___main__",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_isolate_import",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_isolate_bootstrap",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "molt_init_main_molt",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(is_user_owned_symbol(
            "main_molt__helper",
            "app",
            Some(&stdlib_modules)
        ));

        assert!(!is_user_owned_symbol(
            "molt_init_sys",
            "app",
            Some(&stdlib_modules)
        ));
        assert!(!is_user_owned_symbol(
            "molt_init_json",
            "app",
            Some(&stdlib_modules)
        ));
    }

    #[test]
    fn backend_rss_default_scales_with_host_memory() {
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(8 * GIB)),
            4
        );
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(16 * GIB)),
            8
        );
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(32 * GIB)),
            12
        );
        assert_eq!(
            default_backend_max_rss_gb_from_physical_mem_bytes(Some(64 * GIB)),
            16
        );
    }

    #[test]
    fn shared_stdlib_cache_requires_matching_key() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-stdlib-cache-key-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib_path = tmp_dir.join("stdlib.o");
        std::fs::write(&stdlib_path, b"placeholder").expect("write stdlib object");

        write_shared_stdlib_cache_sidecars(
            &stdlib_path,
            7,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            "partition-a",
        )
        .expect("write sidecars");
        assert!(shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));
        assert!(shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            None,
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("def456"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"def456\"}"),
            Some("partition-a"),
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-b"),
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            None,
            Some("partition-a"),
        ));
        assert!(!shared_stdlib_cache_matches(&stdlib_path, None, None, None));

        std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(&stdlib_path))
            .expect("remove partition manifest");
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            None,
        ));
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));

        write_shared_stdlib_cache_sidecars(
            &stdlib_path,
            7,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            "partition-a",
        )
        .expect("rewrite sidecars");
        std::fs::write(&stdlib_path, b"changed-object").expect("mutate object");
        assert!(!shared_stdlib_cache_matches(
            &stdlib_path,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            Some("partition-a"),
        ));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn shared_stdlib_publish_lock_serializes_concurrent_threads() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-stdlib-publish-lock-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib_path = tmp_dir.join("stdlib.o");
        let first_inside = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let violation = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (first_entered_tx, first_entered_rx) = std::sync::mpsc::channel();
        let (second_entered_tx, second_entered_rx) = std::sync::mpsc::channel();

        let first_path = stdlib_path.clone();
        let first_inside_for_first = Arc::clone(&first_inside);
        let first_thread = std::thread::spawn(move || {
            with_shared_stdlib_cache_publish_lock(&first_path, || {
                first_inside_for_first.store(true, std::sync::atomic::Ordering::SeqCst);
                first_entered_tx.send(()).expect("signal first entered");
                std::thread::sleep(std::time::Duration::from_millis(150));
                first_inside_for_first.store(false, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .expect("first lock body");
        });

        first_entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("first thread entered lock");

        let second_path = stdlib_path.clone();
        let first_inside_for_second = Arc::clone(&first_inside);
        let violation_for_second = Arc::clone(&violation);
        let second_thread = std::thread::spawn(move || {
            with_shared_stdlib_cache_publish_lock(&second_path, || {
                if first_inside_for_second.load(std::sync::atomic::Ordering::SeqCst) {
                    violation_for_second.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                second_entered_tx.send(()).expect("signal second entered");
                Ok(())
            })
            .expect("second lock body");
        });

        assert!(
            second_entered_rx
                .recv_timeout(std::time::Duration::from_millis(40))
                .is_err(),
            "second publisher entered while the first publisher held the lock"
        );
        first_thread.join().expect("join first publisher");
        second_entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("second publisher eventually entered");
        second_thread.join().expect("join second publisher");
        assert!(
            !violation.load(std::sync::atomic::Ordering::SeqCst),
            "shared stdlib publish lock allowed overlapping writers"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn shared_stdlib_partition_manifest_tracks_names_and_bodies() {
        let func_a = FunctionIR {
            name: "molt_init_sys".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "return_none".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let func_b = FunctionIR {
            name: "sys__version".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "const_str".to_string(),
                s_value: Some("3.12".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let mut changed = func_b.clone();
        changed.ops[0].s_value = Some("3.13".to_string());

        let ordered = shared_stdlib_partition_manifest(&[func_a.clone(), func_b.clone()])
            .expect("partition manifest");
        let reordered = shared_stdlib_partition_manifest(&[func_b, func_a.clone()])
            .expect("partition manifest");
        let body_changed =
            shared_stdlib_partition_manifest(&[func_a, changed]).expect("partition manifest");

        assert_eq!(ordered, reordered);
        assert_ne!(ordered, body_changed);
        assert!(ordered.contains("\"molt_init_sys\""));
        assert!(ordered.contains("\"sys__version\""));
        assert!(ordered.contains("\"schema\":\"stdlib-partition-v1\""));
    }

    #[test]
    fn shared_stdlib_partition_rejects_unclosed_copy_reference() {
        let userdict_copy = FunctionIR {
            name: "collections__UserDict_copy".to_string(),
            params: vec!["self".to_string()],
            ops: vec![OpIR {
                kind: "call".to_string(),
                s_value: Some("copy__copy".to_string()),
                args: Some(vec!["self".to_string()]),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let copy_init = FunctionIR {
            name: "molt_init_copy".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "call".to_string(),
                s_value: Some("copy__molt_module_chunk_1".to_string()),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let copy_chunk = FunctionIR {
            name: "copy__molt_module_chunk_1".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let copy_copy = FunctionIR {
            name: "copy__copy".to_string(),
            params: vec!["obj".to_string()],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["obj".to_string()]),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let valid_partition = vec![
            userdict_copy.clone(),
            copy_init,
            copy_chunk,
            copy_copy.clone(),
        ];
        let valid_function_names: std::collections::BTreeSet<String> = valid_partition
            .iter()
            .map(|func| func.name.clone())
            .collect();
        validate_shared_stdlib_partition(&valid_partition, &valid_function_names)
            .expect("closed partition");

        let invalid_partition = vec![userdict_copy];
        let invalid_function_names: std::collections::BTreeSet<String> =
            ["collections__UserDict_copy", "copy__copy"]
                .into_iter()
                .map(str::to_string)
                .collect();
        let issue =
            shared_stdlib_partition_closure_issue(&invalid_partition, &invalid_function_names)
                .expect("missing copy reference");
        assert!(issue.contains("collections__UserDict_copy -> copy__copy"));
        assert!(
            validate_shared_stdlib_partition(&invalid_partition, &invalid_function_names).is_err()
        );
    }

    #[test]
    fn shared_stdlib_cache_sidecar_write_failures_propagate() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-stdlib-cache-key-error-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let blocking = tmp_dir.join("not-a-dir");
        std::fs::write(&blocking, b"x").expect("write blocking file");
        let stdlib_path = blocking.join("stdlib.o");

        let err = write_shared_stdlib_cache_sidecars(
            &stdlib_path,
            7,
            Some("abc123"),
            Some("{\"cache_key\":\"abc123\"}"),
            "partition-a",
        )
        .expect_err("sidecar writes should fail when parent is not a directory");
        assert!(!err.to_string().is_empty());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn partition_functions_for_batches_respects_op_budget() {
        let funcs = vec![
            FunctionIR {
                name: "a".to_string(),
                params: vec![],
                ops: vec![Default::default(); 90],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "b".to_string(),
                params: vec![],
                ops: vec![Default::default(); 90],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "c".to_string(),
                params: vec![],
                ops: vec![Default::default(); 10],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ];

        let batches = partition_functions_for_batches(funcs, 64, 100);
        let names: Vec<Vec<String>> = batches
            .into_iter()
            .map(|batch| batch.into_iter().map(|f| f.name).collect())
            .collect();

        assert_eq!(
            names,
            vec![
                vec!["a".to_string()],
                vec!["b".to_string(), "c".to_string()],
            ]
        );
    }

    #[test]
    fn partition_functions_for_batches_respects_count_budget() {
        let funcs = (0..5)
            .map(|idx| FunctionIR {
                name: format!("f{idx}"),
                params: vec![],
                ops: vec![Default::default(); 1],
                param_types: None,
                source_file: None,
                is_extern: false,
            })
            .collect();

        let batches = partition_functions_for_batches(funcs, 2, 1000);
        let sizes: Vec<usize> = batches.into_iter().map(|batch| batch.len()).collect();

        assert_eq!(sizes, vec![2, 2, 1]);
    }

    #[test]
    fn relocatable_linker_binary_prefers_override_then_env() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_molt_linker = std::env::var("MOLT_LINKER").ok();
        let prior_ld = std::env::var("LD").ok();
        let prior_cc = std::env::var("CC").ok();

        unsafe {
            std::env::set_var("MOLT_LINKER", "molt-ld");
            std::env::set_var("LD", "system-ld");
            std::env::set_var("CC", "clang");
        }
        assert_eq!(relocatable_linker_binary(Some("explicit")), "explicit");
        assert_eq!(relocatable_linker_binary(None), "molt-ld");

        unsafe {
            std::env::remove_var("MOLT_LINKER");
        }
        assert_eq!(relocatable_linker_binary(None), "system-ld");

        unsafe {
            std::env::remove_var("LD");
        }
        assert_eq!(relocatable_linker_binary(None), "clang");

        match prior_molt_linker {
            Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
            None => unsafe { std::env::remove_var("MOLT_LINKER") },
        }
        match prior_ld {
            Some(value) => unsafe { std::env::set_var("LD", value) },
            None => unsafe { std::env::remove_var("LD") },
        }
        match prior_cc {
            Some(value) => unsafe { std::env::set_var("CC", value) },
            None => unsafe { std::env::remove_var("CC") },
        }
    }

    #[test]
    fn merge_relocatable_objects_copies_single_input() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-merge-reloc-single-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let input = tmp_dir.join("input.o");
        let output = tmp_dir.join("output.o");
        std::fs::write(&input, b"object-bytes").expect("write input object");

        merge_relocatable_objects(
            &output,
            std::slice::from_ref(&input),
            Some("linker-that-must-not-run"),
        )
        .expect("copy single input object");

        assert_eq!(
            std::fs::read(&output).expect("read merged output"),
            b"object-bytes"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn merge_relocatable_objects_reports_linker_failure() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-merge-reloc-fail-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let input_a = tmp_dir.join("a.o");
        let input_b = tmp_dir.join("b.o");
        let output = tmp_dir.join("output.o");
        std::fs::write(&input_a, b"a").expect("write first input object");
        std::fs::write(&input_b, b"b").expect("write second input object");
        let failing_linker = write_failing_relocatable_linker(&tmp_dir);
        let failing_linker_arg = failing_linker.to_string_lossy();

        let err = merge_relocatable_objects(
            &output,
            &[input_a.clone(), input_b.clone()],
            Some(failing_linker_arg.as_ref()),
        )
        .expect_err("merge should fail with failing linker");
        let message = err.to_string();
        assert!(message.contains("relocatable link failed"), "{message}");
        assert!(!output.exists());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn native_application_object_batches_cleanup_temp_dir_after_merge_failure() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_batch_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
        let prior_linker = std::env::var("MOLT_LINKER").ok();
        let temp_root = std::env::temp_dir();
        let batch_prefix = format!("molt_batch_{}_", std::process::id());
        let before: std::collections::BTreeSet<_> = std::fs::read_dir(&temp_root)
            .expect("read temp root before")
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with(&batch_prefix))
            .collect();
        let tmp_dir = temp_root.join(format!(
            "molt-native-app-merge-fail-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let failing_linker = write_failing_relocatable_linker(&tmp_dir);
        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1");
            std::env::set_var("MOLT_LINKER", failing_linker.as_os_str());
        }

        let output = tmp_dir.join("output.o");
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("helper".to_string()),
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
                    name: "helper".to_string(),
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

        let err = compile_native_application_object_to_path(
            ir,
            &output,
            NativeApplicationObjectOptions {
                target_triple: None,
                stdlib_split_enabled: false,
                app_intrinsic_manifest: None,
                log_prefix: "MOLT_BACKEND(test)",
            },
        )
        .expect_err("forced linker failure should propagate");
        let message = err.to_string();
        assert!(message.contains("relocatable link failed"), "{message}");
        assert!(!output.exists(), "failed merge must not publish output");

        let after: std::collections::BTreeSet<_> = std::fs::read_dir(&temp_root)
            .expect("read temp root after")
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with(&batch_prefix))
            .collect();
        assert_eq!(
            after, before,
            "batch temp dirs must be cleaned after failure"
        );

        match prior_batch_size {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
        }
        match prior_linker {
            Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
            None => unsafe { std::env::remove_var("MOLT_LINKER") },
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn native_application_object_uses_op_budget_even_when_count_fits() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_batch_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
        let prior_op_budget = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET").ok();
        let prior_linker = std::env::var("MOLT_LINKER").ok();
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-native-app-op-budget-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let failing_linker = write_failing_relocatable_linker(&tmp_dir);
        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "64");
            std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", "1");
            std::env::set_var("MOLT_LINKER", failing_linker.as_os_str());
        }

        let output = tmp_dir.join("output.o");
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("helper".to_string()),
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
                    name: "helper".to_string(),
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

        let err = compile_native_application_object_to_path(
            ir,
            &output,
            NativeApplicationObjectOptions {
                target_triple: None,
                stdlib_split_enabled: false,
                app_intrinsic_manifest: None,
                log_prefix: "MOLT_BACKEND(test)",
            },
        )
        .expect_err("op budget must force relocatable batching");
        let message = err.to_string();
        assert!(message.contains("relocatable link failed"), "{message}");
        assert!(!output.exists(), "failed merge must not publish output");

        match prior_batch_size {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
        }
        match prior_op_budget {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET") },
        }
        match prior_linker {
            Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
            None => unsafe { std::env::remove_var("MOLT_LINKER") },
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn resolved_batch_size_and_op_budget_limits_default_and_zero_disable_caps() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
        let prior_ops = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET").ok();

        unsafe {
            std::env::remove_var("MOLT_BACKEND_BATCH_SIZE");
            std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET");
        }
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
            DEFAULT_BACKEND_BATCH_SIZE
        );
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
            DEFAULT_STDLIB_BATCH_SIZE
        );
        assert_eq!(
            resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET),
            DEFAULT_BACKEND_BATCH_OP_BUDGET
        );

        unsafe {
            std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "0");
            std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", "0");
        }
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
            usize::MAX
        );
        assert_eq!(
            resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
            usize::MAX
        );
        assert_eq!(
            resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET),
            usize::MAX
        );

        match prior_size {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
        }
        match prior_ops {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET") },
        }
    }

    #[test]
    fn dead_function_elimination_prunes_stdlib_before_partition() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("molt_init_sys".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_app".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "app__module".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(73),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_json".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(843),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        molt_backend::inject_runtime_exit(&mut ir);
        molt_backend::eliminate_dead_functions(&mut ir);
        molt_backend::eliminate_dead_imports(&mut ir);
        molt_backend::eliminate_dead_ops(&mut ir);
        let retained: std::collections::BTreeSet<_> =
            ir.functions.iter().map(|func| func.name.as_str()).collect();

        assert!(retained.contains("molt_main"));
        assert!(retained.contains("molt_init_sys"));
        assert!(!retained.contains("molt_init_json"));
    }

    #[test]
    fn prune_and_partition_native_stdlib_keeps_only_reachable_stdlib() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("molt_init_sys".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_app".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "app__module".to_string(),
                    params: vec![],
                    ops: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(73),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_json".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(843),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string()]);
        let (user_remaining, stdlib_funcs) =
            prune_and_partition_native_stdlib(&mut ir, "app", Some(&stdlib_modules));
        let user_names: Vec<_> = user_remaining
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();

        assert_eq!(user_names, vec!["molt_main"]);
        assert_eq!(stdlib_names, vec!["molt_init_sys"]);
    }

    #[test]
    fn prune_and_partition_native_stdlib_keeps_non_entry_user_module_in_user_partition() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call".to_string(),
                        s_value: Some("demo__module".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "demo__module".to_string(),
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
                    name: "molt_isolate_import".to_string(),
                    params: vec!["p0".to_string()],
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

        let stdlib_modules = std::collections::BTreeSet::new();
        let (user_remaining, stdlib_funcs) =
            prune_and_partition_native_stdlib(&mut ir, "__main__", Some(&stdlib_modules));
        let user_names: Vec<_> = user_remaining
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();

        assert_eq!(
            user_names,
            vec!["molt_main", "demo__module", "molt_isolate_import"]
        );
        assert!(stdlib_names.is_empty());
    }

    #[test]
    fn compile_stdlib_cache_object_emits_parseable_empty_object() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-empty-stdlib-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib = tmp_dir.join("empty-stdlib.o");

        compile_stdlib_cache_object(&stdlib, Vec::new(), None, None, "MOLT_BACKEND(test)")
            .expect("empty stdlib cache must emit an object");

        let bytes = std::fs::read(&stdlib).expect("read emitted empty stdlib object");
        assert!(
            !bytes.is_empty(),
            "empty stdlib cache path must publish a real object file"
        );
        cranelift_object::object::File::parse(&*bytes)
            .expect("empty stdlib cache must be a parseable object");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn daemon_empty_stdlib_partition_emits_cache_artifact_and_sidecars() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-empty-stdlib-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let stdlib = tmp_dir.join("stdlib.o");
        let runtime_symbols = tmp_dir.join("runtime_intrinsic_symbols.txt");
        std::fs::write(&runtime_symbols, "molt_main\n").expect("write runtime symbols");

        let env_keys = [
            "MOLT_ENTRY_MODULE",
            "MOLT_STDLIB_OBJ",
            "MOLT_STDLIB_CACHE_KEY",
            "MOLT_STDLIB_CACHE_MANIFEST",
            "MOLT_STDLIB_MODULE_SYMBOLS",
            "MOLT_RUNTIME_INTRINSIC_SYMBOLS",
        ];
        let prior_env: Vec<(&str, Option<String>)> = env_keys
            .iter()
            .copied()
            .map(|key| (key, std::env::var(key).ok()))
            .collect();
        unsafe {
            std::env::set_var("MOLT_ENTRY_MODULE", "demo");
            std::env::set_var("MOLT_STDLIB_OBJ", &stdlib);
            std::env::set_var("MOLT_STDLIB_CACHE_KEY", "daemon-empty-key");
            std::env::set_var("MOLT_STDLIB_CACHE_MANIFEST", "daemon-empty-manifest");
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
            std::env::set_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS", &runtime_symbols);
        }

        let job = DaemonJobRequest {
            id: "job0".to_string(),
            is_wasm: false,
            target_triple: None,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_runtime_table_min: None,
            output: output.to_string_lossy().into_owned(),
            cache_key: "".to_string(),
            function_cache_key: None,
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: false,
            ir: Some(SimpleIR {
                functions: vec![
                    FunctionIR {
                        name: "molt_main".to_string(),
                        params: vec![],
                        ops: vec![OpIR {
                            kind: "call".to_string(),
                            s_value: Some("demo__module".to_string()),
                            value: Some(0),
                            ..OpIR::default()
                        }],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                    },
                    FunctionIR {
                        name: "demo__module".to_string(),
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
                        name: "molt_isolate_bootstrap".to_string(),
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
                        name: "molt_isolate_import".to_string(),
                        params: vec!["p0".to_string()],
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
            }),
            ir_path: None,
        };

        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(job, &mut cache);

        for (key, value) in prior_env {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "application output object missing");
        let stdlib_bytes = std::fs::read(&stdlib).expect("read daemon empty stdlib object");
        assert!(
            !stdlib_bytes.is_empty(),
            "daemon empty stdlib cache must publish a real object"
        );
        cranelift_object::object::File::parse(&*stdlib_bytes)
            .expect("daemon empty stdlib cache must be a parseable object");
        assert_eq!(
            std::fs::read_to_string(stdlib_cache_count_sidecar_path(&stdlib))
                .expect("read stdlib count sidecar"),
            "0"
        );
        assert_eq!(
            read_stdlib_cache_key(&stdlib).as_deref(),
            Some("daemon-empty-key")
        );
        assert_eq!(
            read_stdlib_cache_manifest(&stdlib).as_deref(),
            Some("daemon-empty-manifest")
        );
        let partition_manifest =
            std::fs::read_to_string(stdlib_cache_partition_manifest_sidecar_path(&stdlib))
                .expect("read stdlib partition manifest");
        assert!(partition_manifest.contains("\"functions\":[]"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn daemon_native_without_stdlib_obj_keeps_full_ir() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call".to_string(),
                        s_value: Some("demo__module".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "demo__module".to_string(),
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
                    name: "molt_isolate_bootstrap".to_string(),
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
                    name: "molt_isolate_import".to_string(),
                    params: vec!["p0".to_string()],
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

        let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
        let entry_module = std::env::var("MOLT_ENTRY_MODULE").ok();
        unsafe {
            std::env::remove_var("MOLT_STDLIB_OBJ");
            std::env::remove_var("MOLT_ENTRY_MODULE");
        }

        // Mirror the daemon native path: without a stdlib cache target,
        // it must compile the full IR, not the drained remainder.
        let maybe_stdlib = std::env::var("MOLT_STDLIB_OBJ").ok();
        if maybe_stdlib.is_none() {
            molt_backend::inject_runtime_exit(&mut ir);
            molt_backend::eliminate_dead_functions(&mut ir);
            molt_backend::eliminate_dead_imports(&mut ir);
            molt_backend::eliminate_dead_ops(&mut ir);
        }

        let names: Vec<_> = ir.functions.iter().map(|func| func.name.as_str()).collect();

        match stdlib_obj_path {
            Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_OBJ", value) },
            None => unsafe { std::env::remove_var("MOLT_STDLIB_OBJ") },
        }
        match entry_module {
            Some(value) => unsafe { std::env::set_var("MOLT_ENTRY_MODULE", value) },
            None => unsafe { std::env::remove_var("MOLT_ENTRY_MODULE") },
        }

        assert_eq!(
            names,
            vec![
                "molt_main",
                "demo__module",
                "molt_isolate_bootstrap",
                "molt_isolate_import"
            ]
        );
    }

    #[test]
    fn daemon_request_with_env_preserves_user_entry_object() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-request-env-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let stdlib = tmp_dir.join("stdlib.o");
        // The main application object emits the per-app intrinsic resolver, which
        // requires the linked runtime staticlib's `molt_*` intrinsic-symbol set
        // (`MOLT_RUNTIME_INTRINSIC_SYMBOLS`). Production always extracts and
        // exposes this before native codegen; replicate that precondition through
        // the daemon's env-passthrough so this test exercises the real resolver
        // path instead of hitting the fail-closed guard. These IR functions take
        // no intrinsic addresses (no `const_str` ops), so the resolved manifest is
        // empty regardless; the file just satisfies the required-symbol-set
        // contract with the symbols this object actually references.
        let runtime_symbols = tmp_dir.join("runtime_intrinsic_symbols.txt");
        std::fs::write(&runtime_symbols, "molt_init_sys\nmolt_main\n")
            .expect("write runtime intrinsic symbol set");
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
                "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
                "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
                "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
                "MOLT_RUNTIME_INTRINSIC_SYMBOLS": runtime_symbols.to_string_lossy(),
            },
            "jobs": [{
                "id": "job0",
                "is_wasm": false,
                "output": output.to_string_lossy(),
                "cache_key": "",
                "function_cache_key": "",
                "ir": {
                    "functions": [
                        {"name": "molt_main", "params": [], "ops": [{"kind": "call", "s_value": "demo__module", "value": 0}]},
                        {"name": "demo__module", "params": [], "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}, {"kind": "ret_void"}]},
                        {"name": "molt_isolate_bootstrap", "params": [], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_isolate_import", "params": ["p0"], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_init_sys", "params": [], "ops": [{"kind": "ret_void"}]}
                    ],
                    "profile": null
                }
            }]
        });

        let request = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect("parse daemon request");
        assert_eq!(
            std::env::var("MOLT_ENTRY_MODULE").ok().as_deref(),
            Some("demo")
        );
        assert_eq!(
            std::env::var("MOLT_STDLIB_OBJ").ok().as_deref(),
            Some(stdlib.to_string_lossy().as_ref())
        );
        assert_eq!(
            std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok().as_deref(),
            Some("[\"sys\"]")
        );
        let job = request.jobs.expect("jobs").into_iter().next().expect("job");
        let mut partition_ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call".to_string(),
                        s_value: Some("demo__module".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "demo__module".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("molt_init_sys".to_string()),
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
                    name: "molt_isolate_bootstrap".to_string(),
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
                    name: "molt_isolate_import".to_string(),
                    params: vec!["p0".to_string()],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
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
        let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string()]);
        let (user_remaining, stdlib_funcs) =
            prune_and_partition_native_stdlib(&mut partition_ir, "demo", Some(&stdlib_modules));
        let user_names: Vec<_> = user_remaining
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();
        assert_eq!(
            user_names,
            vec![
                "molt_main",
                "demo__module",
                "molt_isolate_bootstrap",
                "molt_isolate_import"
            ]
        );
        assert_eq!(stdlib_names, vec!["molt_init_sys"]);
        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(job, &mut cache);

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "output object missing");
        assert!(
            output.metadata().expect("output metadata").len() > 240,
            "daemon path emitted empty object"
        );

        // The daemon env-passthrough mutated the process environment; clear the
        // resolver symbol-set var so it does not leak into sibling tests that
        // share `ENV_TEST_MUTEX`.
        unsafe { std::env::remove_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS") };
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn daemon_request_env_clears_omitted_stdlib_module_symbols() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"stale\"]");
            std::env::set_var("MOLT_ENTRY_MODULE", "stale_entry");
        }

        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-clear-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
            },
            "jobs": [],
        });

        let parsed = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect("parse daemon request");

        assert_eq!(parsed.version, Some(BACKEND_DAEMON_PROTOCOL_VERSION));
        assert_eq!(
            std::env::var("MOLT_ENTRY_MODULE").ok().as_deref(),
            Some("demo")
        );
        assert!(std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").is_err());
    }

    #[test]
    fn daemon_request_env_clears_omitted_resource_and_trace_keys_between_requests() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let keys = [
            "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
            "MOLT_CLI_MEMORY_AVAILABLE_GB",
            "MOLT_CLI_MEM_AVAILABLE_GB",
            "MOLT_MEMORY_AVAILABLE_GB",
            "MOLT_MEM_AVAILABLE_GB",
            "MOLT_BACKEND_MAX_RSS_GB",
            "MOLT_BACKEND_MEMORY_RESERVE_GB",
            "MOLT_CLI_MEMORY_RESERVE_GB",
            "MOLT_CLI_MEM_RESERVE_GB",
            "MOLT_MEMORY_RESERVE_GB",
            "MOLT_MEM_RESERVE_GB",
            "RAYON_NUM_THREADS",
            "MOLT_TIR_TRACE_FUNC",
        ];
        let prior_env: Vec<_> = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        unsafe {
            for key in keys {
                std::env::set_var(key, "stale");
            }
        }

        let first = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-resource-env-set",
            "env": {
                "MOLT_BACKEND_MEMORY_AVAILABLE_GB": "9",
                "MOLT_BACKEND_MEMORY_RESERVE_GB": "1",
                "RAYON_NUM_THREADS": "3",
                "MOLT_TIR_TRACE_FUNC": "target_func",
            },
            "jobs": [],
        });
        DaemonRequest::from_json_bytes(
            serde_json::to_string(&first)
                .expect("serialize first request")
                .as_bytes(),
        )
        .expect("parse first daemon request");
        assert_eq!(
            std::env::var("MOLT_BACKEND_MEMORY_AVAILABLE_GB")
                .ok()
                .as_deref(),
            Some("9")
        );
        assert_eq!(
            std::env::var("MOLT_BACKEND_MEMORY_RESERVE_GB")
                .ok()
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            std::env::var("RAYON_NUM_THREADS").ok().as_deref(),
            Some("3")
        );
        assert_eq!(
            std::env::var("MOLT_TIR_TRACE_FUNC").ok().as_deref(),
            Some("target_func")
        );

        let second = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-resource-env-clear",
            "env": {},
            "jobs": [],
        });
        DaemonRequest::from_json_bytes(
            serde_json::to_string(&second)
                .expect("serialize second request")
                .as_bytes(),
        )
        .expect("parse second daemon request");

        for key in keys {
            assert!(
                std::env::var(key).is_err(),
                "{key} leaked across daemon requests"
            );
        }
        for (key, value) in prior_env {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }

    #[test]
    fn daemon_request_env_rejects_malformed_stdlib_module_symbols() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"stale\"]");
        }
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-bad-stdlib-symbols-test",
            "env": {
                "MOLT_STDLIB_MODULE_SYMBOLS": "not-json",
            },
            "jobs": [],
        });

        let err = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect_err("malformed stdlib symbol authority must fail closed");
        assert!(
            err.contains("MOLT_STDLIB_MODULE_SYMBOLS must be a JSON array of strings"),
            "unexpected error message: {err}"
        );
        assert!(std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").is_err());
    }

    #[test]
    fn daemon_batch_compile_keeps_user_module_chunk_stub_defined() {
        let _env_guard = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-daemon-batch-chunk-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let output = tmp_dir.join("out.o");
        let stdlib = tmp_dir.join("stdlib.o");
        // The main application object emits the per-app intrinsic resolver, which
        // requires the linked runtime staticlib's `molt_*` intrinsic-symbol set
        // (`MOLT_RUNTIME_INTRINSIC_SYMBOLS`). Production always extracts and
        // exposes this before native codegen; replicate that precondition through
        // the daemon's env-passthrough so this test exercises the real resolver
        // path instead of hitting the fail-closed guard. These IR functions take
        // no intrinsic addresses (no `const_str` ops), so the resolved manifest is
        // empty regardless; the file just satisfies the required-symbol-set
        // contract with the symbols this object actually references.
        let runtime_symbols = tmp_dir.join("runtime_intrinsic_symbols.txt");
        std::fs::write(
            &runtime_symbols,
            "molt_init_sys\nmolt_init_demo\nmolt_main\nmolt_host_init\n",
        )
        .expect("write runtime intrinsic symbol set");
        let request = serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-test",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
                "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
                "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
                "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
                "MOLT_RUNTIME_INTRINSIC_SYMBOLS": runtime_symbols.to_string_lossy(),
                "MOLT_BACKEND_BATCH_SIZE": "1",
            },
            "jobs": [{
                "id": "job0",
                "is_wasm": false,
                "output": output.to_string_lossy(),
                "cache_key": "",
                "function_cache_key": "",
                "ir": {
                    "functions": [
                        {"name": "molt_main", "params": [], "ops": [
                            {"kind": "call", "s_value": "molt_init_demo", "value": 0},
                            {"kind": "ret_void"}
                        ]},
                        {"name": "molt_host_init", "params": [], "ops": [
                            {"kind": "call", "s_value": "molt_init_demo", "value": 0},
                            {"kind": "ret_void"}
                        ]},
                        {"name": "molt_init_demo", "params": [], "ops": [
                            {"kind": "call", "s_value": "demo__molt_module_chunk_1", "value": 0},
                            {"kind": "ret_void"}
                        ]},
                        {"name": "demo__molt_module_chunk_1", "params": [], "ops": [
                            {"kind": "ret_void"}
                        ]},
                        {"name": "molt_isolate_bootstrap", "params": [], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_isolate_import", "params": ["p0"], "ops": [{"kind": "ret_void"}]},
                        {"name": "molt_init_sys", "params": [], "ops": [{"kind": "ret_void"}]}
                    ],
                    "profile": null
                }
            }]
        });

        let request = DaemonRequest::from_json_bytes(
            serde_json::to_string(&request)
                .expect("serialize request")
                .as_bytes(),
        )
        .expect("parse daemon request");
        let job = request.jobs.expect("jobs").into_iter().next().expect("job");
        let mut cache = DaemonCache::new(None);
        let result = compile_single_job(job, &mut cache);

        assert!(result.ok, "daemon compile failed: {:?}", result.message);
        assert!(output.exists(), "output object missing");

        let nm_output = std::process::Command::new("nm")
            .args(["-g", output.to_str().expect("utf8 output path")])
            .output()
            .expect("run nm");
        assert!(
            nm_output.status.success(),
            "nm failed: {}",
            String::from_utf8_lossy(&nm_output.stderr)
        );
        let text = String::from_utf8_lossy(&nm_output.stdout);
        let has_defined_chunk = text.lines().any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 2 {
                return false;
            }
            let sym = fields
                .last()
                .copied()
                .unwrap_or_default()
                .trim_start_matches('_');
            sym == "demo__molt_module_chunk_1" && fields[fields.len().saturating_sub(2)] == "T"
        });
        let has_undefined_chunk = text.lines().any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() != 2 {
                return false;
            }
            let sym = fields
                .last()
                .copied()
                .unwrap_or_default()
                .trim_start_matches('_');
            sym == "demo__molt_module_chunk_1" && fields[0] == "U"
        });

        assert!(has_defined_chunk, "expected defined chunk symbol:\n{text}");
        assert!(
            !has_undefined_chunk,
            "unexpected undefined chunk symbol:\n{text}"
        );

        // The daemon env-passthrough mutated the process environment; clear the
        // resolver symbol-set var so it does not leak into sibling tests that
        // share `ENV_TEST_MUTEX`.
        unsafe { std::env::remove_var("MOLT_RUNTIME_INTRINSIC_SYMBOLS") };
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn batch_external_function_names_excludes_current_batch_symbols() {
        let all_names = std::collections::BTreeSet::from([
            "molt_main".to_string(),
            "demo__module".to_string(),
            "molt_isolate_bootstrap".to_string(),
            "molt_isolate_import".to_string(),
        ]);
        let batch_funcs = vec![
            FunctionIR {
                name: "molt_main".to_string(),
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
                name: "demo__module".to_string(),
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

        let external_names = super::batch_external_function_names(&all_names, &batch_funcs);

        assert_eq!(
            external_names,
            std::collections::BTreeSet::from([
                "molt_isolate_bootstrap".to_string(),
                "molt_isolate_import".to_string(),
            ])
        );
        assert!(!external_names.contains("molt_main"));
        assert!(!external_names.contains("demo__module"));
    }
}
