use super::*;
use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicI64, Ordering},
};

// ---------------------------------------------------------------------------
// Runtime-scoped pattern registry
// ---------------------------------------------------------------------------

pub(super) struct RegexRuntimeState {
    pub(super) next_handle: AtomicI64,
    pub(super) patterns: Mutex<HashMap<i64, CompiledPattern>>,
}

impl RegexRuntimeState {
    pub(super) fn new() -> Self {
        Self {
            next_handle: AtomicI64::new(1),
            patterns: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn clear(&self) {
        self.patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

pub(super) unsafe extern "C" fn regex_runtime_state_init() -> *mut u8 {
    Box::into_raw(Box::new(RegexRuntimeState::new())) as *mut u8
}

pub(super) unsafe extern "C" fn regex_runtime_state_clear(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        (&*(ptr as *const RegexRuntimeState)).clear();
    }
}

pub(super) unsafe extern "C" fn regex_runtime_state_drop(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(ptr as *mut RegexRuntimeState));
    }
}

pub(super) fn regex_state(_py: &CoreGilToken) -> &'static RegexRuntimeState {
    let ptr = crate::bridge::runtime_state_get_or_init(
        b"molt-runtime-regex/patterns/v1",
        regex_runtime_state_init,
        regex_runtime_state_clear,
        regex_runtime_state_drop,
    );
    assert!(
        !ptr.is_null(),
        "molt regex runtime state initialization failed"
    );
    unsafe { &*(ptr as *const RegexRuntimeState) }
}

pub(super) fn re_alloc_handle(_py: &CoreGilToken) -> i64 {
    regex_state(_py).next_handle.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn re_store_pattern(_py: &CoreGilToken, handle: i64, pattern: CompiledPattern) {
    let mut guard = regex_state(_py)
        .patterns
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.insert(handle, pattern);
}
