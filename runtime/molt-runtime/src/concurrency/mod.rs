pub(crate) mod gil;
pub(crate) mod isolates;
pub(crate) mod locks;

#[cfg(test)]
mod panic_contract_tests;

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
///
/// # Panic contract (profile-conditional, by design)
///
/// Every `$body` used with this macro is required to propagate **all
/// Python-level error conditions explicitly** — by setting a pending Python
/// exception (e.g. via `raise_exception`) and returning the runtime error
/// sentinel — and must **never** rely on a Rust panic to surface a recoverable
/// Python error.  Under that contract, the only way `$body` can panic is a
/// genuine runtime *invariant violation* (a compiler/runtime bug: a poisoned
/// lock, a corrupted allocator, an unreachable branch), which is not a
/// recoverable Python exception and for which aborting the process is the
/// correct, fail-closed behavior.
///
/// The macro therefore dispatches on the crate's panic strategy
/// (`cfg(panic = ...)`), which is fixed per build profile:
///
/// * `panic = "unwind"` (dev / dev-fast / release-fast — used in tests and CI):
///   wrap `$body` in `catch_unwind` as a *defense-in-depth* net.  An invariant
///   violation becomes a catchable `RuntimeError` instead of an abort, which
///   keeps developer iteration ergonomic and prevents undefined behavior from
///   an unwind crossing the `extern "C"` boundary.
///
/// * `panic = "abort"` (release-output / wasm-release — the SHIPPED runtime):
///   `catch_unwind` is a documented no-op (the process aborts *at* the panic
///   site, before any unwinding), so wrapping the body would be dead, dishonest
///   code that merely inflates the binary.  We compile the body directly.  This
///   is sound precisely because the panic contract above guarantees the only
///   reachable panics are invariant violations, for which abort is correct.
///   Under this profile the FFI entry point genuinely contains no
///   `catch_unwind` — making the `Cargo.toml` `release-output` claim true.
#[macro_export]
macro_rules! with_gil_entry {
    ($py:ident, $body:block) => {{
        // GilGuard::new() is cfg-dispatched: a real lock on non-wasm32,
        // a zero-cost no-op struct on wasm32.
        let _gil_guard = $crate::concurrency::GilGuard::new();
        let $py = _gil_guard.token();
        let $py = &$py;

        // Profile-conditional FFI-boundary dispatch lives in molt-runtime-core
        // (`with_gil_entry_body!`) so every catching GIL-entry macro shares one
        // `cfg(panic = ...)` selection and cannot drift.  The raise handler
        // below runs only on the `panic = "unwind"` catch path; it raises a
        // `RuntimeError`, skipping allocator/refcount panics (raising into a
        // corrupted allocator would recurse).
        ::molt_runtime_core::with_gil_entry_body!(raise: |__msg| {
            let __is_alloc_panic = __msg.contains("use-after-free")
                || __msg.contains("invalid type_id")
                || __msg.contains("double free")
                || __msg.contains("slab");
            if !__is_alloc_panic {
                let _ = $crate::builtins::exceptions::raise_exception::<u64>(
                    $py,
                    "RuntimeError",
                    __msg,
                );
            }
        }, $body)
    }};
}

/// Like `with_gil_entry!` but omits `catch_unwind` for performance-critical
/// hot paths whose bodies are guaranteed never to panic (all error handling
/// is done via explicit return values, never via unwinding).
///
/// This eliminates the ~10ns-per-call overhead of `catch_unwind` (landing
/// pads, callee-saved register spills, inhibited inlining) while still
/// acquiring the GIL correctly.
///
/// # Safety contract
/// The `$body` must not panic.  All indexing must be bounds-checked, all
/// Option/Result values must be handled explicitly.  If this contract is
/// violated, the process will abort (extern "C" + unwind = UB → abort).
#[macro_export]
macro_rules! with_gil_entry_nopanic {
    ($py:ident, $body:block) => {{
        let _gil_guard = $crate::concurrency::GilGuard::new();
        let $py = _gil_guard.token();
        let $py = &$py;
        $body
    }};
}
