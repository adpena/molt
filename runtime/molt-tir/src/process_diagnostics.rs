#[cfg(unix)]
pub(crate) fn process_peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let raw = unsafe { usage.assume_init().ru_maxrss };
    if raw <= 0 {
        return None;
    }
    let raw = raw as u64;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        Some(raw)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    )))]
    {
        Some(raw.saturating_mul(1024))
    }
}

#[cfg(not(unix))]
pub(crate) fn process_peak_rss_bytes() -> Option<u64> {
    None
}

pub fn process_peak_rss_mib_label() -> String {
    process_peak_rss_bytes()
        .map(|bytes| format!("{:.1}", bytes as f64 / 1_048_576.0))
        .unwrap_or_else(|| "unknown".to_string())
}
