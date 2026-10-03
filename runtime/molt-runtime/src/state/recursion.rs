use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::Ordering;

pub(crate) const DEFAULT_RECURSION_LIMIT: usize = 1000;

thread_local! {
    // Depth belongs to the execution stack, not to the process. A suspended
    // activation carries no charge while another task executes on this thread.
    static RECURSION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[inline]
pub(crate) fn recursion_depth() -> usize {
    RECURSION_DEPTH.with(Cell::get)
}

#[inline]
pub(crate) fn recursion_limit_get() -> usize {
    crate::state::runtime_state::runtime_state_for_gil()
        .expect("recursion guard requires an active runtime")
        .recursion_limit
        .load(Ordering::Relaxed)
}

pub(crate) fn recursion_limit_set(limit: usize) {
    crate::state::runtime_state::runtime_state_for_gil()
        .expect("recursion limit requires an active runtime")
        .recursion_limit
        .store(limit, Ordering::Relaxed);
}

#[inline]
pub(crate) fn recursion_guard_enter() -> bool {
    let limit = recursion_limit_get();
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current >= limit {
            false
        } else {
            depth.set(current + 1);
            true
        }
    })
}

#[inline]
pub(crate) fn recursion_guard_exit() {
    RECURSION_DEPTH.with(|depth| {
        depth.set(
            depth
                .get()
                .checked_sub(1)
                .expect("unbalanced recursion guard exit"),
        );
    });
}

pub(crate) fn assert_thread_recursion_idle() {
    // TLS may already have been destroyed at shutdown. While live, resetting
    // an outstanding charge would conceal broken custody.
    let _ = RECURSION_DEPTH.try_with(|depth| {
        assert_eq!(
            depth.get(),
            0,
            "thread teardown crossed an active recursion guard"
        );
    });
}

/// Releases on every return path and cannot move to another thread's stack.
pub(crate) struct RecursionGuard(PhantomData<Rc<()>>);

impl RecursionGuard {
    #[inline]
    pub(crate) fn enter(py: &crate::PyToken<'_>) -> Option<Self> {
        Self::enter_with_message(py, "maximum recursion depth exceeded")
    }

    #[inline]
    pub(crate) fn enter_with_message(py: &crate::PyToken<'_>, message: &str) -> Option<Self> {
        if recursion_guard_enter() {
            Some(Self(PhantomData))
        } else {
            crate::raise_exception::<u64>(py, "RecursionError", message);
            None
        }
    }
}

impl Drop for RecursionGuard {
    #[inline]
    fn drop(&mut self) {
        recursion_guard_exit();
    }
}

#[cfg(test)]
mod tests;
