// On wasm32, profiling is never used. All profile_* functions are zero-cost no-ops
// so callers see `if false { ... }` and the compiler eliminates the branch entirely
// without any source-level change to call sites.
#[cfg(target_arch = "wasm32")]
mod wasm_stubs {
    use std::sync::atomic::AtomicU64;

    #[inline(always)]
    pub(crate) fn profile_enabled(_py: &crate::PyToken<'_>) -> bool {
        false
    }

    #[inline(always)]
    pub(crate) fn profile_hit(_py: &crate::PyToken<'_>, _counter: &AtomicU64) {}

    #[inline(always)]
    pub(crate) fn profile_hit_unchecked(_counter: &AtomicU64) {}
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm_stubs::{profile_enabled, profile_hit, profile_hit_unchecked};

// Full profiling implementation for non-wasm32 targets.
#[cfg(not(target_arch = "wasm32"))]
mod native {
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
        crate::with_gil_entry!(_py, { if profile_enabled(_py) { 1 } else { 0 } })
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

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_struct_field_store() {
        crate::with_gil_entry!(_py, {
            profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn molt_profile_handle_resolve() {
        crate::with_gil_entry!(_py, {
            profile_hit(_py, &HANDLE_RESOLVE_COUNT);
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{
    molt_profile_enabled, molt_profile_handle_resolve, molt_profile_struct_field_store,
    profile_enabled, profile_hit, profile_hit_unchecked,
};
