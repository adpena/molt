//! Current resident memory of the coordinator process.

#[cfg(target_os = "linux")]
pub fn current_process_rss_bytes() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages = contents
        .split_ascii_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?;
    // SAFETY: sysconf with _SC_PAGESIZE takes no additional arguments.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let bytes = pages.checked_mul(u64::try_from(page_size).ok()?)?;
    (bytes > 0).then_some(bytes)
}

#[cfg(target_os = "macos")]
pub fn current_process_rss_bytes() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo writes at most size bytes into the matching struct.
    let read = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    // SAFETY: proc_pidinfo returned the full size of the record.
    Some(unsafe { info.assume_init() }.pti_resident_size)
}

#[cfg(windows)]
pub fn current_process_rss_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut info = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: the current process pseudo-handle and matching buffer live across the call.
    let read = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &raw mut info, info.cb) };
    (read != 0).then_some(info.WorkingSetSize as u64)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn current_process_rss_bytes() -> Option<u64> {
    None
}
