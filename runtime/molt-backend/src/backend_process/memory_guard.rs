#[cfg(unix)]
use std::env;

#[cfg(any(unix, windows))]
use super::config::default_backend_max_rss_gb;

pub(crate) fn install_process_memory_guard() {
    install_unix_memory_guard();
    install_windows_memory_guard();
}

/// The backend's committed-memory cap in bytes. The memory guard writes
/// `MOLT_BACKEND_MAX_PROCESS_RSS_GB` as fractional GB, so it is read through
/// the one GB parser the TIR pipeline cache uses; a zero or unparsable value
/// falls back to the physical-memory default.
#[cfg(any(unix, windows))]
fn backend_max_rss_bytes() -> u64 {
    backend_max_rss_bytes_from(
        std::env::var("MOLT_BACKEND_MAX_PROCESS_RSS_GB")
            .ok()
            .as_deref(),
        default_backend_max_rss_gb(),
    )
}

fn backend_max_rss_bytes_from(raw_gb: Option<&str>, default_gb: u64) -> u64 {
    raw_gb
        .and_then(molt_passes::memory_budget::parse_nonnegative_gb)
        .filter(|bytes| *bytes > 0)
        .unwrap_or_else(|| default_gb.saturating_mul(1024 * 1024 * 1024))
}

#[cfg(unix)]
fn install_unix_memory_guard() {
    let max_bytes = backend_max_rss_bytes();
    if let Err(reason) = install_committed_memory_rlimit(max_bytes)
        && env::var("MOLT_DEBUG_RLIMIT").as_deref() == Ok("1")
    {
        eprintln!("WARNING: backend memory limit ({max_bytes} bytes) not active: {reason}.");
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
    let max_bytes = backend_max_rss_bytes();
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

#[cfg(test)]
mod tests {
    use super::backend_max_rss_bytes_from;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn fractional_gb_from_the_memory_guard_becomes_the_cap() {
        assert_eq!(
            backend_max_rss_bytes_from(Some("1.500000"), 64),
            3 * GIB / 2
        );
        assert_eq!(backend_max_rss_bytes_from(Some(" 12 "), 64), 12 * GIB);
    }

    #[test]
    fn zero_or_invalid_values_fall_back_to_the_physical_default() {
        for raw in [
            None,
            Some("0"),
            Some("0.0"),
            Some("-1"),
            Some("not-a-number"),
            Some("inf"),
        ] {
            assert_eq!(backend_max_rss_bytes_from(raw, 64), 64 * GIB, "{raw:?}");
        }
    }
}
