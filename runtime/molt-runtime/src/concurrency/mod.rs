pub(crate) mod gil;
pub(crate) mod isolates;
pub(crate) mod locks;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

pub(crate) use gil::{GilGuard, GilReleaseGuard, PyToken, gil_assert, gil_held, with_gil};
#[allow(unused_imports)]
pub(crate) use isolates::*;
#[allow(unused_imports)]
pub(crate) use locks::*;

#[cfg(not(target_arch = "wasm32"))]
static THREAD_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Number of threads that have acquired the GIL at least once.
/// Used by `gil_held()` to fast-path the common single-threaded case.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) static GIL_THREAD_COUNT: AtomicU64 = AtomicU64::new(1);

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static THREAD_ID: u64 = THREAD_ID_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_thread_id() -> u64 {
    THREAD_ID.with(|id| *id)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_thread_id() -> u64 {
    1
}

/// Acquire the GIL and execute `$body` with a `PyToken` bound to `$py`.
///
/// On wasm32 `GilGuard::new()` is a zero-cost no-op (single-threaded target),
/// so this macro compiles down to a direct body invocation with no mutex or
/// TLS overhead.  On all other targets it acquires the real mutex-based GIL.
#[macro_export]
macro_rules! with_gil_entry {
    ($py:ident, $body:block) => {{
        // GilGuard::new() is cfg-dispatched: a real lock on non-wasm32,
        // a zero-cost no-op struct on wasm32.
        let _gil_guard = $crate::concurrency::GilGuard::new();
        let $py = _gil_guard.token();
        let $py = &$py;
        $body
    }};
}
