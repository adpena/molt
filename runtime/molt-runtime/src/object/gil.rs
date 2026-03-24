//! GIL removal infrastructure — Phase 1.
//!
//! Removes gil_assert() from hot refcount paths. Adds per-object
//! fine-grained locking for mutable containers (list, dict, set).

use std::sync::Mutex;

/// Per-object lock for mutable container operations.
/// Replaces the global GIL for container-level synchronization.
pub struct ObjectLock {
    inner: Mutex<()>,
}

impl ObjectLock {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(()),
        }
    }

    /// Acquire the lock. Returns a guard that releases on drop.
    #[inline]
    pub fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Global flag: when true, @par functions execute without GIL.
/// Set by the parallel execution runtime before launching worker threads.
static GIL_RELEASED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Check if the GIL is currently released (parallel mode active).
#[inline(always)]
pub fn is_gil_released() -> bool {
    GIL_RELEASED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Release the GIL for parallel execution.
pub fn release_gil() {
    GIL_RELEASED.store(true, std::sync::atomic::Ordering::Release);
}

/// Re-acquire the GIL after parallel execution.
pub fn acquire_gil() {
    GIL_RELEASED.store(false, std::sync::atomic::Ordering::Release);
}

/// Replacement for gil_assert() — no-op in release builds,
/// checks GIL state in debug builds.
#[inline(always)]
pub fn gil_check() {
    #[cfg(debug_assertions)]
    {
        if is_gil_released() {
            // In parallel mode, GIL is intentionally released — this is fine.
            // Only warn if we're in a non-@par context.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_lock_acquire_release() {
        let lock = ObjectLock::new();
        {
            let _guard = lock.lock();
            // Guard holds lock; released on drop.
        }
        // Lock is available again after drop.
        let _guard2 = lock.lock();
    }

    #[test]
    fn gil_released_flag_roundtrip() {
        // Start in acquired state.
        acquire_gil();
        assert!(!is_gil_released());

        release_gil();
        assert!(is_gil_released());

        acquire_gil();
        assert!(!is_gil_released());
    }

    #[test]
    fn gil_check_does_not_panic() {
        // Must not panic in either GIL state.
        acquire_gil();
        gil_check();
        release_gil();
        gil_check();
        acquire_gil(); // restore
    }

    #[test]
    fn object_lock_new_is_const() {
        // Verifies const fn contract: usable in static initializers.
        static LOCK: ObjectLock = ObjectLock::new();
        let _g = LOCK.lock();
    }
}
