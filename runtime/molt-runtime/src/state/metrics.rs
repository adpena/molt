// On wasm32 we still support logical runtime counters when MOLT_PROFILE=1, but
// RSS sampling remains unavailable in the host-agnostic wasm runtime.

/// RC drop-insertion substrate (design 20): true iff `MOLT_ASSERT_NO_LEAK` is
/// set to a truthy value. When set, the alloc/dealloc profile counters are
/// force-enabled (so the `live = alloc - dealloc` gauge is populated even
/// without `MOLT_PROFILE`), and a process-exit assertion fires if more than
/// `EXPECTED_LIVE_OBJECTS` objects survive. The single source of truth consulted
/// by both the wasm and native `profile_env_enabled`.
fn profile_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|val| !val.is_empty() && val != "0")
        .unwrap_or(false)
}

pub(crate) fn leak_assertion_enabled() -> bool {
    profile_flag_enabled("MOLT_ASSERT_NO_LEAK")
}

/// Sole target-independent authority for enabling runtime counters. WASM and
/// native must not grow separate environment interpretations.
fn profile_env_enabled() -> bool {
    profile_flag_enabled("MOLT_PROFILE") || leak_assertion_enabled()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessMemorySnapshot {
    pub(crate) source: &'static str,
    pub(crate) current_rss_bytes: Option<u64>,
    pub(crate) peak_rss_bytes: Option<u64>,
}

impl ProcessMemorySnapshot {
    #[inline]
    const fn unavailable(source: &'static str) -> Self {
        Self {
            source,
            current_rss_bytes: None,
            peak_rss_bytes: None,
        }
    }

    #[inline]
    fn available(source: &'static str, current_rss_bytes: u64, peak_rss_bytes: u64) -> Self {
        Self {
            source,
            current_rss_bytes: Some(current_rss_bytes),
            peak_rss_bytes: Some(peak_rss_bytes.max(current_rss_bytes)),
        }
    }

    #[inline]
    pub(crate) const fn available_flag(self) -> bool {
        self.current_rss_bytes.is_some() && self.peak_rss_bytes.is_some()
    }
}

/// Phase-0 exact-survivor leak gauge (doc 55 §2.5 / ownership_lattice_phase0.md
/// §2.4). The measured immortal-survivor floor |S| for THIS program's import set,
/// snapshot as `live = ALLOC_COUNT - DEALLOC_COUNT` at the bootstrap->user-code
/// boundary (`molt_runtime_init` "ok"). `u64::MAX` = not snapshot (the assertion
/// falls back to the `EXPECTED_LIVE_OBJECTS` ceiling).
pub(crate) static LIVE_FLOOR: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// One-shot snapshot of the survivor floor. Only records when the leak gauge is
/// enabled (counters are then force-live); idempotent — the first call wins.
pub(crate) fn snapshot_live_floor() {
    if !leak_assertion_enabled() {
        return;
    }
    use std::sync::atomic::Ordering as O;
    let live = crate::ALLOC_COUNT
        .load(O::Relaxed)
        .saturating_sub(crate::DEALLOC_COUNT.load(O::Relaxed));
    let _ = LIVE_FLOOR.compare_exchange(u64::MAX, live, O::Relaxed, O::Relaxed);
}

/// The measured survivor floor, or `None` if no snapshot was taken.
pub(crate) fn live_floor() -> Option<u64> {
    let v = LIVE_FLOOR.load(std::sync::atomic::Ordering::Relaxed);
    if v == u64::MAX { None } else { Some(v) }
}

/// Exact-mode tolerance: `Some(n)` enables the exact-survivor gauge
/// (`live <= floor + n`, catching BOUNDED leaks) for the memory-safety
/// differentials; `None` keeps the default-profile `EXPECTED_LIVE_OBJECTS`
/// ceiling. Set via `MOLT_LEAK_TOLERANCE` (a small slack covering module-level
/// scaffolding, far below the 200K ceiling).
pub(crate) fn leak_exact_tolerance() -> Option<u64> {
    std::env::var("MOLT_LEAK_TOLERANCE")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// The count of immortal bootstrap objects (module dicts, builtin type objects,
/// interned singletons) that legitimately survive to process exit and so are NOT
/// leaks. Measured on a hello-world program at the end of Phase 1 / Phase 3
/// bring-up and encoded here; the leak report subtracts it and
/// `MOLT_ASSERT_NO_LEAK` gates on `live <= EXPECTED_LIVE_OBJECTS`. A program that
/// frees every expression temporary will report `live` at or near this floor.
///
/// This is an UPPER-BOUND ceiling, not an exact equality target: the immortal
/// bootstrap set varies slightly with which stdlib modules a program imports
/// (each module's init allocates a handful of immortal singletons). The ceiling
/// is sized so a non-leaking program passes and a per-iteration leak (which
/// grows `live` without bound) fails decisively.
pub(crate) const EXPECTED_LIVE_OBJECTS: u64 = 200_000;

#[cfg(target_arch = "wasm32")]
mod wasm_stubs {
    use std::sync::atomic::{AtomicU8, AtomicU64, Ordering as AtomicOrdering};

    use crate::{HANDLE_RESOLVE_COUNT, PyToken, STRUCT_FIELD_STORE_COUNT};

    const PROFILE_UNKNOWN: u8 = 2;
    static PROFILE_ENABLED: AtomicU8 = AtomicU8::new(PROFILE_UNKNOWN);

    pub(crate) fn init_profile_enabled_from_env() {
        PROFILE_ENABLED.store(
            u8::from(super::profile_env_enabled()),
            AtomicOrdering::Relaxed,
        );
    }

    pub(crate) fn profile_enabled_unchecked() -> bool {
        match PROFILE_ENABLED.load(AtomicOrdering::Relaxed) {
            0 => false,
            1 => true,
            _ => {
                let enabled = u8::from(super::profile_env_enabled());
                let _ = PROFILE_ENABLED.compare_exchange(
                    PROFILE_UNKNOWN,
                    enabled,
                    AtomicOrdering::Relaxed,
                    AtomicOrdering::Relaxed,
                );
                enabled != 0
            }
        }
    }

    pub(crate) fn profile_enabled(_py: &PyToken<'_>) -> bool {
        profile_enabled_unchecked()
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_enabled() -> u64 {
        if profile_enabled_unchecked() { 1 } else { 0 }
    }

    pub(crate) fn profile_hit(_py: &PyToken<'_>, counter: &AtomicU64) {
        if profile_enabled(_py) {
            counter.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn profile_hit_unchecked(counter: &AtomicU64) {
        if profile_enabled_unchecked() {
            counter.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn profile_hit_bytes_unchecked(counter: &AtomicU64, bytes: u64) {
        if profile_enabled_unchecked() {
            counter.fetch_add(bytes, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn profile_hit_bytes(_py: &PyToken<'_>, counter: &AtomicU64, bytes: u64) {
        if profile_enabled(_py) {
            counter.fetch_add(bytes, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn process_memory_snapshot() -> super::ProcessMemorySnapshot {
        super::ProcessMemorySnapshot::unavailable("unsupported-wasm")
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_snapshot() {
        crate::with_gil_entry_nopanic!(_py, {
            if profile_enabled(_py) {
                let _ = process_memory_snapshot();
            }
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_struct_field_store() {
        crate::with_gil_entry_nopanic!(_py, {
            profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_handle_resolve() {
        crate::with_gil_entry_nopanic!(_py, {
            profile_hit(_py, &HANDLE_RESOLVE_COUNT);
        })
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm_stubs::{
    init_profile_enabled_from_env, molt_profile_enabled, molt_profile_handle_resolve,
    molt_profile_snapshot, molt_profile_struct_field_store, process_memory_snapshot,
    profile_enabled, profile_enabled_unchecked, profile_hit, profile_hit_bytes,
    profile_hit_bytes_unchecked, profile_hit_unchecked,
};

// Full profiling implementation for non-wasm32 targets.
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::atomic::{AtomicU8, AtomicU64, Ordering as AtomicOrdering};

    use crate::{HANDLE_RESOLVE_COUNT, PyToken, STRUCT_FIELD_STORE_COUNT};

    const PROFILE_UNKNOWN: u8 = 2;
    static PROFILE_ENABLED: AtomicU8 = AtomicU8::new(PROFILE_UNKNOWN);

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        #[link_name = "mach_task_self_"]
        static MOLT_MACH_TASK_SELF: libc::mach_port_t;
    }

    pub(crate) fn init_profile_enabled_from_env() {
        PROFILE_ENABLED.store(
            u8::from(super::profile_env_enabled()),
            AtomicOrdering::Relaxed,
        );
    }

    pub(crate) fn profile_enabled_unchecked() -> bool {
        match PROFILE_ENABLED.load(AtomicOrdering::Relaxed) {
            0 => false,
            1 => true,
            _ => {
                let enabled = u8::from(super::profile_env_enabled());
                let _ = PROFILE_ENABLED.compare_exchange(
                    PROFILE_UNKNOWN,
                    enabled,
                    AtomicOrdering::Relaxed,
                    AtomicOrdering::Relaxed,
                );
                enabled != 0
            }
        }
    }

    pub(crate) fn profile_enabled(_py: &PyToken<'_>) -> bool {
        profile_enabled_unchecked()
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_enabled() -> u64 {
        if profile_enabled_unchecked() { 1 } else { 0 }
    }

    pub(crate) fn profile_hit(_py: &PyToken<'_>, counter: &AtomicU64) {
        if profile_enabled(_py) {
            counter.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn profile_hit_unchecked(counter: &AtomicU64) {
        if profile_enabled_unchecked() {
            counter.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn profile_hit_bytes_unchecked(counter: &AtomicU64, bytes: u64) {
        if profile_enabled_unchecked() {
            counter.fetch_add(bytes, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn profile_hit_bytes(_py: &PyToken<'_>, counter: &AtomicU64, bytes: u64) {
        if profile_enabled(_py) {
            counter.fetch_add(bytes, AtomicOrdering::Relaxed);
        }
    }

    #[cfg(target_os = "macos")]
    fn read_process_memory() -> Option<(u64, u64)> {
        unsafe {
            let mut info: libc::mach_task_basic_info = std::mem::zeroed();
            let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
            let kr = libc::task_info(
                MOLT_MACH_TASK_SELF,
                libc::MACH_TASK_BASIC_INFO,
                (&raw mut info).cast(),
                &mut count,
            );
            (kr == 0).then(|| {
                (
                    std::ptr::addr_of!(info.resident_size).read_unaligned(),
                    std::ptr::addr_of!(info.resident_size_max).read_unaligned(),
                )
            })
        }
    }

    #[cfg(target_os = "linux")]
    fn parse_linux_status_memory(contents: &str) -> Option<(u64, u64)> {
        fn kib_value(line: &str, name: &str) -> Option<u64> {
            let value = line.strip_prefix(name)?.split_whitespace().next()?;
            value.parse::<u64>().ok()?.checked_mul(1024)
        }

        let mut current = None;
        let mut peak = None;
        for line in contents.lines() {
            current = current.or_else(|| kib_value(line, "VmRSS:"));
            peak = peak.or_else(|| kib_value(line, "VmHWM:"));
            if current.is_some() && peak.is_some() {
                break;
            }
        }
        Some((current?, peak?))
    }

    #[cfg(target_os = "linux")]
    fn read_process_memory() -> Option<(u64, u64)> {
        let contents = std::fs::read_to_string("/proc/self/status").ok()?;
        parse_linux_status_memory(&contents)
    }

    #[cfg(windows)]
    fn read_process_memory() -> Option<(u64, u64)> {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?,
            ..Default::default()
        };
        let ok =
            unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, counters.cb) };
        (ok != 0).then_some((
            u64::try_from(counters.WorkingSetSize).ok()?,
            u64::try_from(counters.PeakWorkingSetSize).ok()?,
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    fn read_process_memory() -> Option<(u64, u64)> {
        None
    }

    #[inline]
    const fn process_memory_source() -> &'static str {
        #[cfg(target_os = "linux")]
        {
            "proc-self-status"
        }
        #[cfg(target_os = "macos")]
        {
            "mach-task-basic-info"
        }
        #[cfg(windows)]
        {
            "windows-process-memory-info"
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            "unsupported-target"
        }
    }

    pub(crate) fn process_memory_snapshot() -> super::ProcessMemorySnapshot {
        let source = process_memory_source();
        read_process_memory()
            .map(|(current, peak)| super::ProcessMemorySnapshot::available(source, current, peak))
            .unwrap_or_else(|| super::ProcessMemorySnapshot::unavailable(source))
    }

    /// Extern entry point: sample RSS and update peak. Can be called from
    /// compiled code or periodically from the runtime.
    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_snapshot() {
        crate::with_gil_entry_nopanic!(_py, {
            if profile_enabled(_py) {
                let _ = process_memory_snapshot();
            }
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_struct_field_store() {
        crate::with_gil_entry_nopanic!(_py, {
            profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_handle_resolve() {
        crate::with_gil_entry_nopanic!(_py, {
            profile_hit(_py, &HANDLE_RESOLVE_COUNT);
        })
    }

    #[cfg(test)]
    mod tests {
        #[cfg(target_os = "linux")]
        #[test]
        fn linux_status_parser_reads_current_and_high_water_bytes() {
            let status = "Name:\tmolt\nVmHWM:\t4096 kB\nVmRSS:\t3072 kB\n";
            assert_eq!(
                super::parse_linux_status_memory(status),
                Some((3_145_728, 4_194_304))
            );
            assert_eq!(super::parse_linux_status_memory("VmRSS:\t1 kB\n"), None);
        }

        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        #[test]
        fn supported_platform_reports_canonical_rss_source() {
            let snapshot = super::process_memory_snapshot();
            assert_eq!(snapshot.source, super::process_memory_source());
            assert!(snapshot.available_flag());
            assert!(snapshot.peak_rss_bytes >= snapshot.current_rss_bytes);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{
    init_profile_enabled_from_env, molt_profile_enabled, molt_profile_handle_resolve,
    molt_profile_snapshot, molt_profile_struct_field_store, process_memory_snapshot,
    profile_enabled, profile_enabled_unchecked, profile_hit, profile_hit_bytes,
    profile_hit_bytes_unchecked, profile_hit_unchecked,
};
