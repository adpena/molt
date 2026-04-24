// On wasm32 we still support logical runtime counters when MOLT_PROFILE=1, but
// RSS sampling remains unavailable in the host-agnostic wasm runtime.
#[cfg(target_arch = "wasm32")]
mod wasm_stubs {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    use crate::{HANDLE_RESOLVE_COUNT, PyToken, STRUCT_FIELD_STORE_COUNT, runtime_state};

    static PROFILE_ENABLED_GIL_FREE: OnceLock<bool> = OnceLock::new();

    fn profile_enabled_unchecked() -> bool {
        *PROFILE_ENABLED_GIL_FREE.get_or_init(|| {
            std::env::var("MOLT_PROFILE")
                .map(|val| !val.is_empty() && val != "0")
                .unwrap_or(false)
        })
    }

    pub(crate) fn profile_enabled(_py: &PyToken<'_>) -> bool {
        *runtime_state(_py).profile_enabled.get_or_init(|| {
            std::env::var("MOLT_PROFILE")
                .map(|val| !val.is_empty() && val != "0")
                .unwrap_or(false)
        })
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
    current_rss_bytes, molt_profile_enabled, molt_profile_handle_resolve, molt_profile_snapshot,
    molt_profile_struct_field_store, profile_enabled, profile_hit, profile_hit_bytes,
    profile_hit_unchecked, sample_peak_rss,
};

// Full profiling implementation for non-wasm32 targets.
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    use crate::{
        HANDLE_RESOLVE_COUNT, PEAK_RSS_BYTES, PyToken, STRUCT_FIELD_STORE_COUNT, runtime_state,
    };

    static PROFILE_ENABLED_GIL_FREE: OnceLock<bool> = OnceLock::new();

    fn profile_enabled_unchecked() -> bool {
        *PROFILE_ENABLED_GIL_FREE.get_or_init(|| {
            std::env::var("MOLT_PROFILE")
                .map(|val| !val.is_empty() && val != "0")
                .unwrap_or(false)
        })
    }

    pub(crate) fn profile_enabled(_py: &PyToken<'_>) -> bool {
        *runtime_state(_py).profile_enabled.get_or_init(|| {
            std::env::var("MOLT_PROFILE")
                .map(|val| !val.is_empty() && val != "0")
                .unwrap_or(false)
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_enabled() -> u64 {
        // Fast path: check the OnceLock without acquiring the GIL.
        // profile_enabled is set once at startup and never changes.
        if let Some(state) = crate::state::runtime_state::runtime_state_for_gil() {
            let enabled = state.profile_enabled.get_or_init(|| {
                std::env::var("MOLT_PROFILE")
                    .map(|val| !val.is_empty() && val != "0")
                    .unwrap_or(false)
            });
            if *enabled { 1 } else { 0 }
        } else {
            0
        }
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
    current_rss_bytes, molt_profile_enabled, molt_profile_handle_resolve, molt_profile_snapshot,
    molt_profile_struct_field_store, profile_enabled, profile_hit, profile_hit_bytes,
    profile_hit_unchecked, sample_peak_rss,
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
