use super::*;
use crate::state::pending_work::PendingWorkKind;

#[test]
fn stopped_pool_publication_notifies_the_actual_execution_word() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let runtime = crate::runtime_state(py);
        schedule_stopped_pool(&runtime.concurrent, i64::MAX);
        let word = unsafe { &*runtime.pending_work.word_ptr() };
        assert_ne!(
            word.load(Ordering::Relaxed) & PendingWorkKind::PoolRetirement as u32,
            0
        );
        // Stale worklist identities are real producer inputs too. The bounded
        // consumer must remove one without inventing a pool or running a callback.
        crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(py);
        assert!(!concurrent_retirement_pending(&runtime.concurrent));
        assert_eq!(
            word.load(Ordering::Relaxed) & PendingWorkKind::PoolRetirement as u32,
            0
        );
    });
}

#[test]
fn stopped_poll_retry_republishes_the_word_after_nested_service_clears_it() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let runtime = crate::runtime_state(py);
        let state = &runtime.concurrent;
        // Model an outer poll owning the removed FIFO identity. Its retry drop
        // is the real producer under test, including unwind paths.
        let poll = StoppedPoolPoll {
            state,
            id: i64::MAX,
            retry: true,
        };
        state.publish_stopped_pending(false);
        runtime
            .pending_work
            .serviced(PendingWorkKind::PoolRetirement, || false);
        drop(poll);
        let word = unsafe { &*runtime.pending_work.word_ptr() };
        assert_ne!(
            word.load(Ordering::Relaxed) & PendingWorkKind::PoolRetirement as u32,
            0
        );
        crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(py);
        assert!(!concurrent_retirement_pending(state));
    });
}
