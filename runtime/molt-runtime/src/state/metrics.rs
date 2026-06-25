// On wasm32 we still support logical runtime counters when MOLT_PROFILE=1, but
// RSS sampling remains unavailable in the host-agnostic wasm runtime.

/// RC drop-insertion substrate (design 20): true iff `MOLT_ASSERT_NO_LEAK` is
/// set to a truthy value. When set, the alloc/dealloc profile counters are
/// force-enabled (so the `live = alloc - dealloc` gauge is populated even
/// without `MOLT_PROFILE`), and a process-exit assertion fires if more than
/// `EXPECTED_LIVE_OBJECTS` objects survive. The single source of truth consulted
/// by both the wasm and native `profile_env_enabled`.
pub(crate) fn leak_assertion_enabled() -> bool {
    std::env::var("MOLT_ASSERT_NO_LEAK")
        .map(|val| !val.is_empty() && val != "0")
        .unwrap_or(false)
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

    fn profile_env_enabled() -> bool {
        let direct = std::env::var("MOLT_PROFILE")
            .map(|val| !val.is_empty() && val != "0")
            .unwrap_or(false);
        // MOLT_ASSERT_NO_LEAK force-enables counting so the leak gauge is live.
        direct || super::leak_assertion_enabled()
    }

    pub(crate) fn init_profile_enabled_from_env() {
        PROFILE_ENABLED.store(u8::from(profile_env_enabled()), AtomicOrdering::Relaxed);
    }

    fn profile_enabled_unchecked() -> bool {
        match PROFILE_ENABLED.load(AtomicOrdering::Relaxed) {
            0 => false,
            1 => true,
            _ => {
                let enabled = u8::from(profile_env_enabled());
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

    pub(crate) fn profile_hit_bytes(_py: &PyToken<'_>, counter: &AtomicU64, bytes: u64) {
        if profile_enabled(_py) {
            counter.fetch_add(bytes, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn current_rss_bytes() -> u64 {
        0
    }

    pub(crate) fn sample_peak_rss() {}

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_snapshot() {
        crate::with_gil_entry_nopanic!(_py, {
            if profile_enabled(_py) {
                sample_peak_rss();
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
    current_rss_bytes, init_profile_enabled_from_env, molt_profile_enabled,
    molt_profile_handle_resolve, molt_profile_snapshot, molt_profile_struct_field_store,
    profile_enabled, profile_hit, profile_hit_bytes, profile_hit_unchecked, sample_peak_rss,
};

// Full profiling implementation for non-wasm32 targets.
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::atomic::{AtomicU8, AtomicU64, Ordering as AtomicOrdering};

    use crate::{HANDLE_RESOLVE_COUNT, PEAK_RSS_BYTES, PyToken, STRUCT_FIELD_STORE_COUNT};

    const PROFILE_UNKNOWN: u8 = 2;
    static PROFILE_ENABLED: AtomicU8 = AtomicU8::new(PROFILE_UNKNOWN);

    fn profile_env_enabled() -> bool {
        let direct = std::env::var("MOLT_PROFILE")
            .map(|val| !val.is_empty() && val != "0")
            .unwrap_or(false);
        // MOLT_ASSERT_NO_LEAK force-enables counting so the leak gauge is live.
        direct || super::leak_assertion_enabled()
    }

    pub(crate) fn init_profile_enabled_from_env() {
        PROFILE_ENABLED.store(u8::from(profile_env_enabled()), AtomicOrdering::Relaxed);
    }

    fn profile_enabled_unchecked() -> bool {
        match PROFILE_ENABLED.load(AtomicOrdering::Relaxed) {
            0 => false,
            1 => true,
            _ => {
                let enabled = u8::from(profile_env_enabled());
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

    pub(crate) fn profile_hit_bytes(_py: &PyToken<'_>, counter: &AtomicU64, bytes: u64) {
        if profile_enabled(_py) {
            counter.fetch_add(bytes, AtomicOrdering::Relaxed);
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn current_rss_bytes() -> u64 {
        // Use mach_task_basic_info to query RSS on macOS.
        #[repr(C)]
        struct MachTaskBasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time: [u32; 2],   // time_value_t
            system_time: [u32; 2], // time_value_t
            policy: i32,
            suspend_count: i32,
        }
        const MACH_TASK_BASIC_INFO: u32 = 20;
        // MACH_TASK_BASIC_INFO_COUNT = size_of::<MachTaskBasicInfo>() / size_of::<u32>()
        const INFO_COUNT: u32 =
            (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;
        unsafe extern "C" {
            fn mach_task_self() -> u32;
            fn task_info(
                target_task: u32,
                flavor: u32,
                task_info_out: *mut MachTaskBasicInfo,
                task_info_count: *mut u32,
            ) -> i32;
        }
        unsafe {
            let mut info: MachTaskBasicInfo = std::mem::zeroed();
            let mut count = INFO_COUNT;
            let kr = task_info(
                mach_task_self(),
                MACH_TASK_BASIC_INFO,
                &mut info,
                &mut count,
            );
            if kr == 0 { info.resident_size } else { 0 }
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn current_rss_bytes() -> u64 {
        // Read /proc/self/statm; second field is RSS in pages.
        if let Ok(contents) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages_str) = contents.split_whitespace().nth(1) {
                if let Ok(rss_pages) = rss_pages_str.parse::<u64>() {
                    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
                    if page_size > 0 {
                        return rss_pages * (page_size as u64);
                    }
                    return rss_pages * 4096;
                }
            }
        }
        0
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(crate) fn current_rss_bytes() -> u64 {
        0
    }

    /// Sample current RSS and update PEAK_RSS_BYTES if it's a new peak.
    pub(crate) fn sample_peak_rss() {
        let rss = current_rss_bytes();
        if rss > 0 {
            // CAS loop to update peak.
            loop {
                let prev = PEAK_RSS_BYTES.load(AtomicOrdering::Relaxed);
                if rss <= prev {
                    break;
                }
                if PEAK_RSS_BYTES
                    .compare_exchange_weak(
                        prev,
                        rss,
                        AtomicOrdering::Relaxed,
                        AtomicOrdering::Relaxed,
                    )
                    .is_ok()
                {
                    break;
                }
            }
        }
    }

    /// Extern entry point: sample RSS and update peak. Can be called from
    /// compiled code or periodically from the runtime.
    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_snapshot() {
        crate::with_gil_entry_nopanic!(_py, {
            if profile_enabled(_py) {
                sample_peak_rss();
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{
    current_rss_bytes, init_profile_enabled_from_env, molt_profile_enabled,
    molt_profile_handle_resolve, molt_profile_snapshot, molt_profile_struct_field_store,
    profile_enabled, profile_hit, profile_hit_bytes, profile_hit_unchecked, sample_peak_rss,
};

/// Mirrors `mach_task_basic_info` from `<mach/task_info.h>`.
#[cfg(target_os = "macos")]
#[repr(C)]
#[allow(dead_code)]
pub(crate) struct MachTaskBasicInfo {
    pub virtual_size: u64,
    pub resident_size: u64,
    pub resident_size_max: u64,
    pub user_time: [u32; 2],
    pub system_time: [u32; 2],
}

#[cfg(target_os = "macos")]
const _: () = {
    assert!(
        core::mem::size_of::<MachTaskBasicInfo>() == 40,
        "MachTaskBasicInfo layout mismatch — Apple may have changed the struct"
    );
};
