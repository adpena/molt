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

#[macro_export]
macro_rules! with_gil_entry {
    ($py:ident, $body:block) => {{
        let _gil_guard = $crate::concurrency::GilGuard::new();
        let $py = _gil_guard.token();
        let $py = &$py;
        $body
    }};
}
