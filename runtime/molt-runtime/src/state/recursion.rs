use crate::{RECURSION_DEPTH, RECURSION_LIMIT};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Global atomic recursion depth counter — avoids TLS on the hot path.
/// For single-threaded programs, atomic increment/decrement with Relaxed
/// ordering is essentially free (compiles to a single add/sub instruction).
static FAST_RECURSION_DEPTH: AtomicUsize = AtomicUsize::new(0);
static FAST_RECURSION_LIMIT: AtomicUsize =
    AtomicUsize::new(crate::state::tls::DEFAULT_RECURSION_LIMIT);

pub(crate) fn recursion_limit_get() -> usize {
    FAST_RECURSION_LIMIT.load(Ordering::Relaxed)
}

pub(crate) fn recursion_limit_set(limit: usize) {
    FAST_RECURSION_LIMIT.store(limit, Ordering::Relaxed);
    // Also update TLS for backward compatibility with code that reads it directly
    RECURSION_LIMIT.with(|cell| cell.set(limit));
}

pub(crate) fn recursion_guard_enter() -> bool {
    let limit = FAST_RECURSION_LIMIT.load(Ordering::Relaxed);
    let current = FAST_RECURSION_DEPTH.fetch_add(1, Ordering::Relaxed);
    if current + 1 > limit {
        // Undo the increment
        FAST_RECURSION_DEPTH.fetch_sub(1, Ordering::Relaxed);
        false
    } else {
        true
    }
}

pub(crate) fn recursion_guard_exit() {
    let prev = FAST_RECURSION_DEPTH.fetch_sub(1, Ordering::Relaxed);
    if prev == 0 {
        // Underflow protection: restore to 0
        FAST_RECURSION_DEPTH.store(0, Ordering::Relaxed);
    }
}

/// Fast-path enter: single atomic fetch_add, no TLS.
#[inline(always)]
pub(crate) fn recursion_guard_enter_fast() -> bool {
    let limit = FAST_RECURSION_LIMIT.load(Ordering::Relaxed);
    let current = FAST_RECURSION_DEPTH.fetch_add(1, Ordering::Relaxed);
    if current + 1 > limit {
        FAST_RECURSION_DEPTH.fetch_sub(1, Ordering::Relaxed);
        false
    } else {
        true
    }
}

/// Fast-path exit: single atomic fetch_sub.
#[inline(always)]
pub(crate) fn recursion_guard_exit_fast() {
    FAST_RECURSION_DEPTH.fetch_sub(1, Ordering::Relaxed);
}

pub(crate) fn sync_fast_depth_to_tls() {
    let depth = FAST_RECURSION_DEPTH.load(Ordering::Relaxed);
    RECURSION_DEPTH.with(|cell| cell.set(depth));
}
