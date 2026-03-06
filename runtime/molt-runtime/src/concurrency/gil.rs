#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex, MutexGuard};

#[cfg(not(target_arch = "wasm32"))]
use crate::{GIL_DEPTH, runtime_state_for_gil};

// ---------------------------------------------------------------------------
// wasm32: single-threaded target — the GIL is always held, all operations
// are no-ops.  We keep the public types so call-sites compile unchanged.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub(crate) struct GilGuard {
    _marker: (),
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct PyToken<'gil> {
    _guard: &'gil GilGuard,
}

#[cfg(target_arch = "wasm32")]
impl GilGuard {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        Self { _marker: () }
    }

    #[inline(always)]
    pub(crate) fn token(&self) -> PyToken<'_> {
        PyToken { _guard: self }
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for GilGuard {
    #[inline(always)]
    fn drop(&mut self) {
        // no-op: single-threaded, no lock to release
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct GilReleaseGuard {
    _marker: (),
}

#[cfg(target_arch = "wasm32")]
impl GilReleaseGuard {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        Self { _marker: () }
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for GilReleaseGuard {
    #[inline(always)]
    fn drop(&mut self) {
        // no-op: single-threaded, no lock to restore
    }
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn gil_held() -> bool {
    // On wasm32 the GIL is logically always held (single-threaded).
    true
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn hold_runtime_gil(_guard: GilGuard) {
    // no-op
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn release_runtime_gil() {
    // no-op
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn with_gil<F, R>(f: F) -> R
where
    F: for<'gil> FnOnce(PyToken<'gil>) -> R,
{
    let guard = GilGuard::new();
    let token = guard.token();
    f(token)
}

// ---------------------------------------------------------------------------
// Non-wasm32: full mutex-based GIL implementation
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
static PREINIT_GIL: Mutex<()> = Mutex::new(());

#[cfg(not(target_arch = "wasm32"))]
fn molt_gil() -> &'static Mutex<()> {
    if let Some(state) = runtime_state_for_gil() {
        &state.gil
    } else {
        &PREINIT_GIL
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn lock_molt_gil() -> MutexGuard<'static, ()> {
    // Panics while holding the GIL should fail the originating operation, but
    // they must not permanently brick unrelated runtime entrypoints or test
    // teardown with a poisoned mutex.
    molt_gil()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct GilGuard {
    _marker: (),
    fallback_guard: Option<MutexGuard<'static, ()>>,
    fallback_depth: bool,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct PyToken<'gil> {
    _guard: &'gil GilGuard,
}

#[cfg(not(target_arch = "wasm32"))]
impl GilGuard {
    pub(crate) fn new() -> Self {
        let needs_lock = match GIL_DEPTH.try_with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            current == 0
        }) {
            Ok(needs_lock) => needs_lock,
            Err(_) => return Self::fallback_new(),
        };
        if needs_lock {
            let guard = lock_molt_gil();
            let stored = GIL_GUARD
                .try_with(|slot| {
                    *slot.borrow_mut() = Some(guard);
                })
                .is_ok();
            if !stored {
                let _ = GIL_DEPTH.try_with(|depth| {
                    let current = depth.get();
                    depth.set(current.saturating_sub(1));
                });
                return Self::fallback_new();
            }
        }
        Self {
            _marker: (),
            fallback_guard: None,
            fallback_depth: false,
        }
    }

    pub(crate) fn token(&self) -> PyToken<'_> {
        PyToken { _guard: self }
    }

    fn fallback_new() -> Self {
        let tid = fallback_thread_id();
        let owner = GIL_FALLBACK_OWNER.load(AtomicOrdering::Acquire);
        if owner == tid {
            GIL_FALLBACK_DEPTH.fetch_add(1, AtomicOrdering::AcqRel);
            return Self {
                _marker: (),
                fallback_guard: None,
                fallback_depth: true,
            };
        }
        let guard = lock_molt_gil();
        GIL_FALLBACK_OWNER.store(tid, AtomicOrdering::Release);
        GIL_FALLBACK_DEPTH.store(1, AtomicOrdering::Release);
        Self {
            _marker: (),
            fallback_guard: Some(guard),
            fallback_depth: true,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for GilGuard {
    fn drop(&mut self) {
        if self.fallback_depth {
            let depth = GIL_FALLBACK_DEPTH.fetch_sub(1, AtomicOrdering::AcqRel);
            let next = depth.saturating_sub(1);
            if next == 0 {
                GIL_FALLBACK_OWNER.store(0, AtomicOrdering::Release);
                let _ = self.fallback_guard.take();
            }
            return;
        }
        let should_release = match GIL_DEPTH.try_with(|depth| {
            let current = depth.get();
            let next = current.saturating_sub(1);
            depth.set(next);
            next == 0
        }) {
            Ok(should_release) => should_release,
            Err(_) => return,
        };
        if should_release {
            let _ = GIL_GUARD.try_with(|slot| {
                let _ = slot.borrow_mut().take();
            });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct GilReleaseGuard {
    depth: usize,
    had_runtime_guard: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl GilReleaseGuard {
    pub(crate) fn new() -> Self {
        let depth = match GIL_DEPTH.try_with(|d| d.get()) {
            Ok(depth) => depth,
            Err(_) => {
                return Self {
                    depth: 0,
                    had_runtime_guard: false,
                };
            }
        };
        if depth == 0 {
            return Self {
                depth: 0,
                had_runtime_guard: false,
            };
        }
        if GIL_DEPTH.try_with(|d| d.set(0)).is_err() {
            return Self {
                depth: 0,
                had_runtime_guard: false,
            };
        }
        let _ = GIL_GUARD.try_with(|slot| {
            let _ = slot.borrow_mut().take();
        });
        let had_runtime_guard = RUNTIME_GIL_GUARD
            .try_with(|slot| slot.borrow_mut().take().is_some())
            .unwrap_or(false);
        Self {
            depth,
            had_runtime_guard,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for GilReleaseGuard {
    fn drop(&mut self) {
        if self.depth == 0 {
            return;
        }
        if self.had_runtime_guard {
            hold_runtime_gil(GilGuard::new());
            let _ = GIL_DEPTH.try_with(|d| d.set(self.depth));
            return;
        }
        let guard = lock_molt_gil();
        let stored = GIL_GUARD
            .try_with(|slot| {
                *slot.borrow_mut() = Some(guard);
            })
            .is_ok();
        if stored {
            let _ = GIL_DEPTH.try_with(|d| d.set(self.depth));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gil_held() -> bool {
    match GIL_DEPTH.try_with(|depth| depth.get()) {
        Ok(depth) => depth > 0 || fallback_gil_held(),
        Err(_) => fallback_gil_held(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static GIL_GUARD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
    static RUNTIME_GIL_GUARD: RefCell<Option<GilGuard>> = const { RefCell::new(None) };
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn hold_runtime_gil(guard: GilGuard) {
    RUNTIME_GIL_GUARD.with(|slot| {
        *slot.borrow_mut() = Some(guard);
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn release_runtime_gil() {
    RUNTIME_GIL_GUARD.with(|slot| {
        let _ = slot.borrow_mut().take();
    });
}

#[cfg(not(target_arch = "wasm32"))]
static GIL_FALLBACK_OWNER: AtomicU64 = AtomicU64::new(0);
#[cfg(not(target_arch = "wasm32"))]
static GIL_FALLBACK_DEPTH: AtomicUsize = AtomicUsize::new(0);

#[cfg(not(target_arch = "wasm32"))]
fn fallback_thread_id() -> u64 {
    let thread_id = std::thread::current().id();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&thread_id, &mut hasher);
    let mut value = std::hash::Hasher::finish(&hasher);
    if value == 0 {
        value = 1;
    }
    value
}

#[cfg(not(target_arch = "wasm32"))]
fn fallback_gil_held() -> bool {
    let owner = GIL_FALLBACK_OWNER.load(AtomicOrdering::Acquire);
    if owner == 0 {
        return false;
    }
    let depth = GIL_FALLBACK_DEPTH.load(AtomicOrdering::Acquire);
    owner == fallback_thread_id() && depth > 0
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn with_gil<F, R>(f: F) -> R
where
    F: for<'gil> FnOnce(PyToken<'gil>) -> R,
{
    let guard = GilGuard::new();
    let token = guard.token();
    f(token)
}

// ---------------------------------------------------------------------------
// gil_assert: available on both targets
// ---------------------------------------------------------------------------

#[cfg(feature = "molt_debug_gil")]
pub(crate) fn gil_assert() {
    assert!(gil_held(), "GIL required for runtime mutation");
}

#[cfg(not(feature = "molt_debug_gil"))]
pub(crate) fn gil_assert() {
    debug_assert!(gil_held(), "GIL required for runtime mutation");
}

// ---------------------------------------------------------------------------
// Tests (non-wasm32 only — they rely on threads and the mutex-based GIL)
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{GilGuard, gil_held};
    use crate::GIL_DEPTH;

    #[test]
    fn gil_depth_tracks_nesting() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let start = GIL_DEPTH.with(|depth| depth.get());
        assert_eq!(gil_held(), start > 0);

        {
            let _g1 = GilGuard::new();
            let depth1 = GIL_DEPTH.with(|depth| depth.get());
            assert_eq!(depth1, start + 1);
            assert!(gil_held());
            {
                let _g2 = GilGuard::new();
                let depth2 = GIL_DEPTH.with(|depth| depth.get());
                assert_eq!(depth2, start + 2);
                assert!(gil_held());
            }
            let depth1_after = GIL_DEPTH.with(|depth| depth.get());
            assert_eq!(depth1_after, start + 1);
        }

        let final_depth = GIL_DEPTH.with(|depth| depth.get());
        assert_eq!(final_depth, start);
        assert_eq!(gil_held(), start > 0);
    }

    #[test]
    fn gil_release_guard_drops_runtime_lock_temporarily() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        super::release_runtime_gil();
        GIL_DEPTH.with(|depth| depth.set(0));

        super::hold_runtime_gil(GilGuard::new());
        assert!(
            super::RUNTIME_GIL_GUARD
                .try_with(|slot| slot.borrow().is_some())
                .unwrap_or(false),
            "runtime GIL guard should be installed before release"
        );
        assert!(
            super::GIL_GUARD
                .try_with(|slot| slot.borrow().is_some())
                .unwrap_or(false),
            "lock guard should be installed before release"
        );

        let release = super::GilReleaseGuard::new();
        assert!(
            !super::RUNTIME_GIL_GUARD
                .try_with(|slot| slot.borrow().is_some())
                .unwrap_or(false),
            "runtime GIL guard should be removed while GilReleaseGuard is active",
        );
        assert!(
            !super::GIL_GUARD
                .try_with(|slot| slot.borrow().is_some())
                .unwrap_or(false),
            "lock guard should be removed while GilReleaseGuard is active",
        );
        assert_eq!(GIL_DEPTH.with(|depth| depth.get()), 0);

        drop(release);
        assert!(
            super::RUNTIME_GIL_GUARD
                .try_with(|slot| slot.borrow().is_some())
                .unwrap_or(false),
            "runtime GIL guard should be restored after GilReleaseGuard drops",
        );
        assert!(
            super::GIL_GUARD
                .try_with(|slot| slot.borrow().is_some())
                .unwrap_or(false),
            "lock guard should be restored after GilReleaseGuard drops",
        );
        assert_eq!(GIL_DEPTH.with(|depth| depth.get()), 1);
        super::release_runtime_gil();
        GIL_DEPTH.with(|depth| depth.set(0));
    }

    #[test]
    fn gil_recovers_after_poisoned_holder_panics() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let worker = std::thread::spawn(|| {
            let _gil = GilGuard::new();
            panic!("poison the GIL mutex");
        });
        assert!(
            worker.join().is_err(),
            "worker panic should poison the mutex"
        );

        let recovered = GilGuard::new();
        assert!(gil_held(), "GIL should still be acquirable after poison");
        drop(recovered);
    }
}
