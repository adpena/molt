use crate::{RECURSION_DEPTH, RECURSION_LIMIT};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Global recursion depth/limit for the fast path (single-threaded hot loop).
/// These shadow the TLS Cells for the common case where there is only one
/// thread executing compiled code. The fast-path functions only touch these
/// globals (two Relaxed atomic ops per enter/exit). The TLS versions are
/// synced lazily: only when something outside the hot path reads the TLS
/// (traceback formatting, sys.getrecursionlimit, etc.).
static FAST_RECURSION_DEPTH: AtomicUsize = AtomicUsize::new(0);
static FAST_RECURSION_LIMIT: AtomicUsize = AtomicUsize::new(1000);

pub(crate) fn recursion_limit_get() -> usize {
    RECURSION_LIMIT.with(|limit| limit.get())
}

pub(crate) fn recursion_limit_set(limit: usize) {
    RECURSION_LIMIT.with(|cell| cell.set(limit));
    FAST_RECURSION_LIMIT.store(limit, Ordering::Relaxed);
}

pub(crate) fn recursion_guard_enter() -> bool {
    let limit = recursion_limit_get();
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current + 1 > limit {
            false
        } else {
            let next = current + 1;
            depth.set(next);
            FAST_RECURSION_DEPTH.store(next, Ordering::Relaxed);
            true
        }
    })
}

pub(crate) fn recursion_guard_exit() {
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            let next = current - 1;
            depth.set(next);
            FAST_RECURSION_DEPTH.store(next, Ordering::Relaxed);
        }
    });
}

/// Ultra-fast recursion guard enter using global atomics only.
/// Avoids ALL TLS overhead — two Relaxed atomic loads + one store (~3ns).
/// The TLS depth is synced lazily by `sync_fast_depth_to_tls`.
/// Returns true if the call is allowed, false if the limit is exceeded.
#[inline(always)]
pub(crate) fn recursion_guard_enter_fast() -> bool {
    let current = FAST_RECURSION_DEPTH.load(Ordering::Relaxed);
    let limit = FAST_RECURSION_LIMIT.load(Ordering::Relaxed);
    if current >= limit {
        // Sync TLS before the slow path reads it
        RECURSION_DEPTH.with(|depth| depth.set(current));
        false
    } else {
        FAST_RECURSION_DEPTH.store(current + 1, Ordering::Relaxed);
        true
    }
}

/// Ultra-fast recursion guard exit using global atomics only.
#[inline(always)]
pub(crate) fn recursion_guard_exit_fast() {
    let current = FAST_RECURSION_DEPTH.load(Ordering::Relaxed);
    if current > 0 {
        FAST_RECURSION_DEPTH.store(current - 1, Ordering::Relaxed);
    }
}

/// Sync the fast global depth back to TLS. Called before anything that
/// reads the TLS recursion depth (e.g. traceback formatting, exception
/// raising, sys.getrecursionlimit/depth).
#[inline]
pub(crate) fn sync_fast_depth_to_tls() {
    let depth = FAST_RECURSION_DEPTH.load(Ordering::Relaxed);
    RECURSION_DEPTH.with(|d| d.set(depth));
}
