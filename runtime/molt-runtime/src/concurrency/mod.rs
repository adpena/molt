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

// GIL_THREAD_COUNT is defined in gil.rs with per-thread tracking.

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

        // Wrap in catch_unwind to prevent panics from unwinding through
        // extern "C" boundaries, which is undefined behavior in Rust.
        match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| $body)) {
            Ok(val) => val,
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic in FFI boundary".to_string()
                };
                // Only attempt to raise a Python exception if the panic
                // was NOT from the allocation/refcount system itself.
                // Otherwise raise_exception would re-enter the corrupted
                // allocator, trigger another panic, and cause infinite
                // recursion via catch_unwind → raise_exception → panic → ...
                let is_alloc_panic = msg.contains("use-after-free")
                    || msg.contains("invalid type_id")
                    || msg.contains("double free")
                    || msg.contains("slab");
                if !is_alloc_panic {
                    let _ = $crate::builtins::exceptions::raise_exception::<u64>(
                        $py,
                        "RuntimeError",
                        &msg,
                    );
                }
                // SAFETY: All FFI return types used with this macro (u64, i64,
                // i32, f64, *mut u8, *const u8, bool, ()) are safely zero-
                // initializable. The caller will check for the pending
                // exception before using this value.
                #[allow(unused_unsafe)]
                unsafe {
                    ::std::mem::zeroed()
                }
            }
        }
    }};
}
