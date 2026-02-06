use std::cell::RefCell;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Mutex, MutexGuard};

use crate::{runtime_state_for_gil, GIL_DEPTH};

static PREINIT_GIL: Mutex<()> = Mutex::new(());

fn molt_gil() -> &'static Mutex<()> {
    if let Some(state) = runtime_state_for_gil() {
        &state.gil
    } else {
        &PREINIT_GIL
    }
}

pub(crate) struct GilGuard {
    _marker: (),
    #[cfg(not(target_arch = "wasm32"))]
    fallback_guard: Option<MutexGuard<'static, ()>>,
    #[cfg(not(target_arch = "wasm32"))]
    fallback_depth: bool,
}

pub(crate) struct PyToken<'gil> {
    _guard: &'gil GilGuard,
}

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
            let guard = molt_gil().lock().unwrap();
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
            #[cfg(not(target_arch = "wasm32"))]
            fallback_guard: None,
            #[cfg(not(target_arch = "wasm32"))]
            fallback_depth: false,
        }
    }

    pub(crate) fn token(&self) -> PyToken<'_> {
        PyToken { _guard: self }
    }

    #[cfg(not(target_arch = "wasm32"))]
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
        let guard = molt_gil().lock().unwrap();
        GIL_FALLBACK_OWNER.store(tid, AtomicOrdering::Release);
        GIL_FALLBACK_DEPTH.store(1, AtomicOrdering::Release);
        Self {
            _marker: (),
            fallback_guard: Some(guard),
            fallback_depth: true,
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn fallback_new() -> Self {
        Self { _marker: () }
    }
}

impl Drop for GilGuard {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
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

pub(crate) struct GilReleaseGuard {
    depth: usize,
}

impl GilReleaseGuard {
    pub(crate) fn new() -> Self {
        let depth = match GIL_DEPTH.try_with(|d| d.get()) {
            Ok(depth) => depth,
            Err(_) => return Self { depth: 0 },
        };
        if depth == 0 {
            return Self { depth: 0 };
        }
        if GIL_DEPTH.try_with(|d| d.set(0)).is_err() {
            return Self { depth: 0 };
        }
        let _ = GIL_GUARD.try_with(|slot| {
            let _ = slot.borrow_mut().take();
        });
        Self { depth }
    }
}

impl Drop for GilReleaseGuard {
    fn drop(&mut self) {
        if self.depth == 0 {
            return;
        }
        let guard = molt_gil().lock().unwrap();
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

pub(crate) fn gil_held() -> bool {
    match GIL_DEPTH.try_with(|depth| depth.get()) {
        Ok(depth) => depth > 0 || fallback_gil_held(),
        Err(_) => fallback_gil_held(),
    }
}

thread_local! {
    static GIL_GUARD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
    static RUNTIME_GIL_GUARD: RefCell<Option<GilGuard>> = const { RefCell::new(None) };
}

pub(crate) fn hold_runtime_gil(guard: GilGuard) {
    RUNTIME_GIL_GUARD.with(|slot| {
        *slot.borrow_mut() = Some(guard);
    });
}

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

#[cfg(target_arch = "wasm32")]
fn fallback_thread_id() -> u64 {
    1
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

#[cfg(target_arch = "wasm32")]
fn fallback_gil_held() -> bool {
    false
}

#[cfg(feature = "molt_debug_gil")]
pub(crate) fn gil_assert() {
    assert!(gil_held(), "GIL required for runtime mutation");
}

#[cfg(not(feature = "molt_debug_gil"))]
pub(crate) fn gil_assert() {
    debug_assert!(gil_held(), "GIL required for runtime mutation");
}

pub(crate) fn with_gil<F, R>(f: F) -> R
where
    F: for<'gil> FnOnce(PyToken<'gil>) -> R,
{
    let guard = GilGuard::new();
    let token = guard.token();
    f(token)
}

#[cfg(test)]
mod tests {
    use super::{gil_held, GilGuard};
    use crate::GIL_DEPTH;

    #[test]
    fn gil_depth_tracks_nesting() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
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
}
