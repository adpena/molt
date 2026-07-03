// Windows bin-test builds compile Unix daemon protocol code for parser coverage
// without running the daemon loop; production warning policy remains unchanged.
#![cfg_attr(all(test, windows), allow(dead_code))]

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "native-backend")]
use molt_backend::SimpleBackend;
use molt_backend::SimpleIR;
#[cfg(feature = "wasm-backend")]
use molt_backend::{WasmBackend, WasmCompileOptions};
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
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, UnlockFileEx};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::System::IO::OVERLAPPED;

mod backend_output;
mod backend_process;
mod backend_request;
mod fact_graph_emit;
use backend_process::*;
use backend_request::BackendCliRequest;

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
    "MOLT_RUNTIME_CALLABLE_SYMBOLS",
    "MOLT_DEBUG_DROP",
    "MOLT_DEBUG_LOWER_FUNC",
    "MOLT_TIR_DUMP",
];

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
    let request = BackendCliRequest::parse(&args)?;
    #[cfg(feature = "native-backend")]
    if let Some(job_file) = request.native_batch_job_file {
        let output_file = request.output_path.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--output is required with --native-batch-job-file",
            )
        })?;
        compile_native_batch_object_job_file(Path::new(job_file), Path::new(output_file))?;
        return Ok(());
    }

    // Read and parse IR. Drop the raw buffer immediately after
    // deserialization to avoid holding two copies in memory simultaneously.
    let document = request.read_document()?;
    backend_output::emit_backend_output_for_request(&request, document)
}

#[cfg(test)]
mod main_tests;
