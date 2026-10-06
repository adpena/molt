#[cfg(any(unix, windows))]
use std::env;

#[cfg(any(unix, windows))]
use super::config::default_backend_max_rss_gb;

pub(crate) fn install_process_memory_guard() {
    install_unix_memory_guard();
    install_windows_memory_guard();
}

#[cfg(unix)]
fn install_unix_memory_guard() {
    let max_gb: u64 = env::var("MOLT_BACKEND_MAX_PROCESS_RSS_GB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(default_backend_max_rss_gb);
    if let Err(reason) = install_committed_memory_rlimit(max_gb * 1024 * 1024 * 1024)
        && env::var("MOLT_DEBUG_RLIMIT").as_deref() == Ok("1")
    {
        eprintln!("WARNING: backend memory limit ({max_gb}GB) not active: {reason}.");
    }
}

/// Linux charges `RLIMIT_DATA` for every writable private mapping, the
/// committed-memory counterpart of the Windows job memory limit below.
/// `RLIMIT_AS` would also charge sparse reservations (allocator arenas, the
/// mapped executable, guard regions) that are not memory.
#[cfg(target_os = "linux")]
fn install_committed_memory_rlimit(max_bytes: u64) -> Result<(), String> {
    let rlim = libc::rlimit {
        rlim_cur: max_bytes,
        rlim_max: max_bytes,
    };
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

#[cfg(not(unix))]
fn install_unix_memory_guard() {}

#[cfg(windows)]
fn install_windows_memory_guard() {
    let max_gb: u64 = env::var("MOLT_BACKEND_MAX_PROCESS_RSS_GB")
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

#[cfg(not(windows))]
fn install_windows_memory_guard() {}
