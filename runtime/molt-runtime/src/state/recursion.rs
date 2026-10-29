use crate::BuiltinExc;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::Ordering;

pub(crate) const DEFAULT_RECURSION_LIMIT: usize = 1000;

// Depth belongs to the execution stack, not to the process: it lives in the
// thread record (`ExecutionTls::recursion_depth`), as CPython keeps it on the
// thread state. A suspended activation carries no charge while another task
// executes on this thread.

#[inline]
pub(crate) fn recursion_depth() -> usize {
    crate::state::execution_tls().recursion_depth.get()
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
    let depth = &crate::state::execution_tls().recursion_depth;
    let current = depth.get();
    if current >= limit {
        false
    } else {
        depth.set(current + 1);
        true
    }
}

#[inline]
pub(crate) fn recursion_guard_exit() {
    let depth = &crate::state::execution_tls().recursion_depth;
    depth.set(
        depth
            .get()
            .checked_sub(1)
            .expect("unbalanced recursion guard exit"),
    );
}

pub(crate) fn assert_thread_recursion_idle() {
    // The thread record has no destructor, so it is readable at shutdown.
    // Resetting an outstanding charge would conceal broken custody.
    assert_eq!(
        recursion_depth(),
        0,
        "thread teardown crossed an active recursion guard"
    );
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
            crate::raise_exception::<u64>(py, BuiltinExc::RecursionError, message);
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
