use crate::{RECURSION_DEPTH, RECURSION_LIMIT};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Global recursion depth — exported as a data symbol for direct access
/// from Cranelift-generated code. The codegen emits load/store instructions
/// against this address instead of calling functions.
#[unsafe(no_mangle)]
pub static molt_fast_recursion_depth: AtomicUsize = AtomicUsize::new(0);

/// Global recursion limit — exported as a data symbol for direct access.
#[unsafe(no_mangle)]
pub static molt_fast_recursion_limit: AtomicUsize = AtomicUsize::new(1000);

// Aliases for internal use.
fn fast_recursion_depth() -> &'static AtomicUsize { &molt_fast_recursion_depth }
fn fast_recursion_limit() -> &'static AtomicUsize { &molt_fast_recursion_limit }

pub(crate) fn recursion_limit_get() -> usize {
    RECURSION_LIMIT.with(|limit| limit.get())
}

pub(crate) fn recursion_limit_set(limit: usize) {
    RECURSION_LIMIT.with(|cell| cell.set(limit));
    molt_fast_recursion_limit.store(limit, Ordering::Relaxed);
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
            molt_fast_recursion_depth.store(next, Ordering::Relaxed);
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
            molt_fast_recursion_depth.store(next, Ordering::Relaxed);
        }
    });
}

/// Ultra-fast recursion guard enter using global atomics only.
/// Avoids ALL TLS overhead — two Relaxed atomic loads + one store (~3ns).
/// The TLS depth is synced lazily by `sync_fast_depth_to_tls`.
/// Returns true if the call is allowed, false if the limit is exceeded.
#[inline(always)]
pub(crate) fn recursion_guard_enter_fast() -> bool {
    let current = molt_fast_recursion_depth.load(Ordering::Relaxed);
    let limit = molt_fast_recursion_limit.load(Ordering::Relaxed);
    if current >= limit {
        // Sync TLS before the slow path reads it
        RECURSION_DEPTH.with(|depth| depth.set(current));
        false
    } else {
        molt_fast_recursion_depth.store(current + 1, Ordering::Relaxed);
        true
    }
}

/// Ultra-fast recursion guard exit using global atomics only.
#[inline(always)]
pub(crate) fn recursion_guard_exit_fast() {
    let current = molt_fast_recursion_depth.load(Ordering::Relaxed);
    if current > 0 {
        molt_fast_recursion_depth.store(current - 1, Ordering::Relaxed);
    }
}

/// Sync the fast global depth back to TLS. Called before anything that
/// reads the TLS recursion depth (e.g. traceback formatting, exception
/// raising, sys.getrecursionlimit/depth).
#[inline]
pub(crate) fn sync_fast_depth_to_tls() {
    let depth = molt_fast_recursion_depth.load(Ordering::Relaxed);
    RECURSION_DEPTH.with(|d| d.set(depth));
}

/// Returns a pointer to the global recursion depth counter.
/// The Cranelift codegen uses this to inline the recursion guard
/// as a load + increment + store instead of a function call.
#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_depth_ptr() -> u64 {
    molt_fast_recursion_depth.as_ptr() as u64
}

/// Returns a pointer to the global recursion limit.
#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_limit_ptr() -> u64 {
    molt_fast_recursion_limit.as_ptr() as u64
}
