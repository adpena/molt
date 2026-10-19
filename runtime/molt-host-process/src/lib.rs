//! Host-tool process budgets, independent of compiler IR and runtime allocations.

const GIB_BYTES: u64 = 1024 * 1024 * 1024;

/// Parse a non-negative, possibly fractional GB value into bytes, saturating
/// at `u64::MAX`.
pub fn parse_nonnegative_gb(raw: &str) -> Option<u64> {
    let gb = raw
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)?;
    Some((gb * GIB_BYTES as f64).min(u64::MAX as f64) as u64)
}

pub fn default_process_max_rss_gb_from_physical_mem_bytes(bytes: Option<u64>) -> u64 {
    match bytes.map(|raw| raw / GIB_BYTES).unwrap_or(0) {
        gib if gib >= 64 => 16,
        gib if gib >= 32 => 12,
        gib if gib >= 16 => 8,
        _ => 4,
    }
}

#[cfg(unix)]
pub fn detect_physical_memory_bytes() -> Option<u64> {
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
pub fn detect_physical_memory_bytes() -> Option<u64> {
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
pub fn detect_physical_memory_bytes() -> Option<u64> {
    None
}

pub fn default_process_max_rss_gb() -> u64 {
    default_process_max_rss_gb_from_physical_mem_bytes(detect_physical_memory_bytes())
}
/// Resolve the admitted process cap; zero and invalid values retain the host default.
pub fn process_max_rss_bytes_from(raw_gb: Option<&str>, default_gb: u64) -> u64 {
    raw_gb
        .and_then(parse_nonnegative_gb)
        .filter(|bytes| *bytes > 0)
        .unwrap_or_else(|| default_gb.saturating_mul(GIB_BYTES))
}

/// Install the committed-memory cap for one host process without changing its
/// inherited hard ceiling. Other Unix kernels retain the parent RSS guard.
pub fn install_process_memory_limit(max_bytes: u64, debug_rlimit: bool) {
    #[cfg(unix)]
    if let Err(reason) = install_committed_memory_rlimit(max_bytes)
        && debug_rlimit
    {
        eprintln!("WARNING: process memory limit ({max_bytes} bytes) not active: {reason}.");
    }
    #[cfg(windows)]
    install_windows_memory_limit(max_bytes);
    #[cfg(not(unix))]
    let _ = debug_rlimit;
    #[cfg(not(any(unix, windows)))]
    let _ = max_bytes;
}

/// Linux charges `RLIMIT_DATA` for every writable private mapping, the
/// committed-memory counterpart of the Windows job memory limit below.
/// `RLIMIT_AS` would also charge sparse reservations (allocator arenas, the
/// mapped executable, guard regions) that are not memory.
///
/// The guard sets only the soft limit and keeps the inherited hard limit, so
/// a daemon can widen its own envelope when it serves several builds.
#[cfg(target_os = "linux")]
fn install_committed_memory_rlimit(max_bytes: u64) -> Result<(), String> {
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` writes only into the `rlimit` it is handed.
    if unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut current) } != 0 {
        return Err(format!(
            "getrlimit(RLIMIT_DATA) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let rlim = libc::rlimit {
        rlim_cur: max_bytes.min(current.rlim_max),
        rlim_max: current.rlim_max,
    };
    // SAFETY: `setrlimit` reads only the `rlimit` it is handed.
    if unsafe { libc::setrlimit(libc::RLIMIT_DATA, &rlim) } != 0 {
        return Err(format!(
            "setrlimit(RLIMIT_DATA) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Other Unix kernels have no committed-memory rlimit (macOS's `RLIMIT_DATA`
/// governs only `brk`); the parent's RSS guard remains the enforcement.
#[cfg(all(unix, not(target_os = "linux")))]
fn install_committed_memory_rlimit(_max_bytes: u64) -> Result<(), String> {
    Err("this kernel has no committed-memory rlimit".to_string())
}

#[cfg(windows)]
fn install_windows_memory_limit(max_bytes: u64) {
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

pub mod host_process_environment_generated;

#[cfg(test)]
mod tests;
