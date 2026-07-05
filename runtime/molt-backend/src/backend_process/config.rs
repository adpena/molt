pub(crate) const DEFAULT_BACKEND_BATCH_SIZE: usize = 64;
pub(crate) const DEFAULT_STDLIB_BATCH_SIZE: usize = 128;
pub(crate) const DEFAULT_BACKEND_BATCH_OP_BUDGET: usize = 8_000;
pub(crate) const MIB: usize = 1024 * 1024;
pub(crate) const GIB: u64 = 1024 * 1024 * 1024;
pub(crate) const DEFAULT_DAEMON_REQUEST_LIMIT_BYTES: usize = 512 * MIB;
pub(crate) const DEFAULT_STDIN_REQUEST_LIMIT_BYTES: usize = DEFAULT_DAEMON_REQUEST_LIMIT_BYTES;

#[cfg(any(unix, test))]
pub(crate) const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;
#[cfg(any(unix, test))]
pub(crate) const DEFAULT_DAEMON_MAX_JOBS: usize = 512;
#[cfg(any(unix, test))]
pub(crate) const DAEMON_REQUEST_ENV_KEYS: &[&str] = &[
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
    // Optimization-pass instruments. Every optimization lands with a
    // firing/refusal instrument; those instruments are useless if the daemon
    // strips their env keys.
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

pub(crate) fn default_backend_max_rss_gb_from_physical_mem_bytes(bytes: Option<u64>) -> u64 {
    match bytes.map(|raw| raw / GIB).unwrap_or(0) {
        gib if gib >= 64 => 16,
        gib if gib >= 32 => 12,
        gib if gib >= 16 => 8,
        _ => 4,
    }
}

#[cfg(unix)]
pub(crate) fn detect_physical_memory_bytes() -> Option<u64> {
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
pub(crate) fn detect_physical_memory_bytes() -> Option<u64> {
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
pub(crate) fn detect_physical_memory_bytes() -> Option<u64> {
    None
}

pub(crate) fn default_backend_max_rss_gb() -> u64 {
    default_backend_max_rss_gb_from_physical_mem_bytes(detect_physical_memory_bytes())
}
