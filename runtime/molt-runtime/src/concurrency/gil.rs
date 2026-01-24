use std::sync::Mutex;

use crate::{runtime_state, GIL_DEPTH};

fn molt_gil() -> &'static Mutex<()> {
    &runtime_state().gil
}

pub(crate) struct GilGuard {
    guard: Option<std::sync::MutexGuard<'static, ()>>,
}

impl GilGuard {
    pub(crate) fn new() -> Self {
        let needs_lock = GIL_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            current == 0
        });
        let guard = if needs_lock {
            Some(molt_gil().lock().unwrap())
        } else {
            None
        };
        Self { guard }
    }
}

impl Drop for GilGuard {
    fn drop(&mut self) {
        let should_release = GIL_DEPTH.with(|depth| {
            let current = depth.get();
            let next = current.saturating_sub(1);
            depth.set(next);
            next == 0
        });
        if should_release {
            self.guard.take();
        }
    }
}
