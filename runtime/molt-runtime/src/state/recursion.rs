use crate::BuiltinExc;
use std::sync::atomic::Ordering;

pub(crate) const DEFAULT_RECURSION_LIMIT: usize = 1000;

// Depth belongs to the execution stack, not to the process: it lives in the
// thread record (`ExecutionTls::recursion_depth`), as CPython keeps it on the
// thread state. A suspended activation carries no charge while another task
// executes on this thread.

#[inline]
pub(crate) fn recursion_depth(py: &crate::PyToken<'_>) -> usize {
    py.execution_tls().recursion_depth.get()
}

#[inline]
pub(crate) fn recursion_limit_get(py: &crate::PyToken<'_>) -> usize {
    crate::state::runtime_state::admitted_runtime_state_in(py.execution_tls())
        .expect("recursion accounting requires its active runtime")
        .recursion_limit
        .load(Ordering::Relaxed)
}

pub(crate) fn recursion_limit_set(py: &crate::PyToken<'_>, limit: usize) {
    crate::state::runtime_state::admitted_runtime_state_in(py.execution_tls())
        .expect("recursion accounting requires its active runtime")
        .recursion_limit
        .store(limit, Ordering::Relaxed);
}

#[inline]
pub(crate) fn recursion_guard_enter(py: &crate::PyToken<'_>) -> bool {
    let limit = recursion_limit_get(py);
    let depth = &py.execution_tls().recursion_depth;
    let current = depth.get();
    if current >= limit {
        false
    } else {
        depth.set(current + 1);
        true
    }
}

#[inline]
pub(crate) fn recursion_guard_exit(py: &crate::PyToken<'_>) {
    release_record_charge(py.execution_tls());
}

#[inline]
fn release_record_charge(record: &crate::state::ExecutionTls) {
    let depth = &record.recursion_depth;
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
        crate::state::execution_tls().recursion_depth.get(),
        0,
        "thread teardown crossed an active recursion guard"
    );
}

/// Releases on every return path and cannot move to another thread's stack.
pub(crate) struct RecursionGuard<'py> {
    record: &'py crate::state::ExecutionTls,
}

impl<'py> RecursionGuard<'py> {
    #[inline]
    pub(crate) fn enter(py: &'py crate::PyToken<'_>) -> Option<Self> {
        Self::enter_with_message(py, "maximum recursion depth exceeded")
    }

    #[inline]
    pub(crate) fn enter_with_message(py: &'py crate::PyToken<'_>, message: &str) -> Option<Self> {
        if recursion_guard_enter(py) {
            Some(Self {
                record: py.execution_tls(),
            })
        } else {
            crate::raise_exception::<u64>(py, BuiltinExc::RecursionError, message);
            None
        }
    }
}

impl Drop for RecursionGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        release_record_charge(self.record);
    }
}

/// Read-only test observation at a genuine C callback boundary; it executes
/// no Python and cannot enter, release or repair a recursion charge.
#[cfg(test)]
pub(crate) fn current_thread_recursion_depth() -> usize {
    crate::state::execution_tls().recursion_depth.get()
}

#[cfg(test)]
mod tests;
