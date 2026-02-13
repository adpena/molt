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
