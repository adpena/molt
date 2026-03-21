pub(crate) mod gil;
pub(crate) mod isolates;
pub(crate) mod locks;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering as AtomicOrdering};

pub(crate) use gil::{GilGuard, GilReleaseGuard, PyToken, gil_assert, gil_held, with_gil};
#[allow(unused_imports)]
pub(crate) use isolates::*;
#[allow(unused_imports)]
pub(crate) use locks::*;

// ---- Single-threaded GIL bypass ------------------------------------------
// Tracks how many threads may acquire the GIL.  Starts at 1 (the main
// thread).  When count == 1 the `with_gil_entry!` macro skips all TLS /
// mutex work and constructs a zero-cost token directly — saving ~10-20
// cycles per runtime call.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) static GIL_THREAD_COUNT: AtomicU32 = AtomicU32::new(1);

/// Call when spawning a thread that will acquire the GIL.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn register_gil_thread() {
    GIL_THREAD_COUNT.fetch_add(1, AtomicOrdering::Release);
}

/// Call when a GIL-capable thread exits.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn unregister_gil_thread() {
    GIL_THREAD_COUNT.fetch_sub(1, AtomicOrdering::Release);
}

// WASM is single-threaded — GIL thread tracking is a no-op.
#[cfg(target_arch = "wasm32")]
#[inline]
pub(crate) fn register_gil_thread() {}

#[cfg(target_arch = "wasm32")]
#[inline]
pub(crate) fn unregister_gil_thread() {}

#[cfg(not(target_arch = "wasm32"))]
static THREAD_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

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
///
/// **Single-threaded fast path (native):** when `GIL_THREAD_COUNT == 1` (the
/// common case — no worker threads have been spawned) we skip TLS lookup and
/// mutex acquisition entirely and construct a lightweight `GilGuard` directly.
/// A single `Relaxed` atomic load (~1 cycle) replaces the old TLS + RefCell +
/// conditional-mutex path (~10-20 cycles), yielding significant speedups on
/// benchmarks with millions of runtime calls.
#[macro_export]
macro_rules! with_gil_entry {
    ($py:ident, $body:block) => {{
        // On wasm32 the guard is already zero-cost; on native targets we
        // check the atomic thread counter for a fast bypass.
        #[cfg(target_arch = "wasm32")]
        let _gil_guard = $crate::concurrency::GilGuard::new();

        #[cfg(not(target_arch = "wasm32"))]
        let _gil_guard = {
            if $crate::concurrency::GIL_THREAD_COUNT
                .load(::std::sync::atomic::Ordering::Relaxed) == 1
            {
                // Single-threaded fast path: no contention possible, skip
                // TLS depth tracking and mutex entirely.
                $crate::concurrency::GilGuard::new_unchecked()
            } else {
                // Multi-threaded: full GIL protocol with TLS + mutex.
                $crate::concurrency::GilGuard::new()
            }
        };

        let $py = _gil_guard.token();
        let $py = &$py;
        $body
    }};
}
