use std::cell::RefCell;
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
            Err(_) => return Self { _marker: () },
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
            }
        }
        Self { _marker: () }
    }

    pub(crate) fn token(&self) -> PyToken<'_> {
        PyToken { _guard: self }
    }
}

impl Drop for GilGuard {
    fn drop(&mut self) {
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
    GIL_DEPTH.try_with(|depth| depth.get() > 0).unwrap_or(false)
}

thread_local! {
    static GIL_GUARD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
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
