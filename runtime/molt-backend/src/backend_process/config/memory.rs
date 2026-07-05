use super::constants::GIB;

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
