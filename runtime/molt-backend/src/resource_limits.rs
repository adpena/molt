pub(crate) const GIB: u64 = 1024 * 1024 * 1024;

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

fn default_backend_max_rss_gb() -> u64 {
    default_backend_max_rss_gb_from_physical_mem_bytes(detect_physical_memory_bytes())
}

fn configured_backend_max_rss_gb() -> u64 {
    std::env::var("MOLT_BACKEND_MAX_RSS_GB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(default_backend_max_rss_gb)
}

pub(crate) fn apply_backend_memory_limit() {
    apply_platform_backend_memory_limit(configured_backend_max_rss_gb());
}

#[cfg(unix)]
fn apply_platform_backend_memory_limit(max_gb: u64) {
    let max_bytes = max_gb * GIB;
    unsafe {
        let rlim = libc::rlimit {
            rlim_cur: max_bytes,
            rlim_max: max_bytes,
        };
        if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0
            && std::env::var("MOLT_DEBUG_RLIMIT").as_deref() == Ok("1")
        {
            eprintln!(
                "WARNING: failed to set memory limit (RLIMIT_AS={max_gb}GB). OOM guard not active."
            );
        }
    }
}

#[cfg(windows)]
fn apply_platform_backend_memory_limit(max_gb: u64) {
    let max_bytes = max_gb * GIB;
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

#[cfg(not(any(unix, windows)))]
fn apply_platform_backend_memory_limit(_max_gb: u64) {}
