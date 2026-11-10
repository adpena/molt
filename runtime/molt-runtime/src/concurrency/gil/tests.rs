use super::{GilGuard, gil_held};
use crate::state::execution_tls;
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

/// Every live guard owns one compositional depth unit; the final 1->0
/// transition releases the mutex regardless of drop order.
#[test]
fn gil_nested_guards_are_compositional() {
    let start = execution_tls().gil_depth.get();
    {
        // Outermost guard performs the 0->1 transition and takes the mutex.
        let _g1 = GilGuard::new();
        let depth1 = execution_tls().gil_depth.get();
        assert_eq!(depth1, start + 1, "outer guard owns the depth gate");
        assert!(gil_held());
        {
            // Re-entry increments custody without relocking the mutex.
            let _g2 = GilGuard::new();
            let depth2 = execution_tls().gil_depth.get();
            assert_eq!(depth2, depth1 + 1);
            assert!(gil_held());
            {
                // Deeper nesting adds another independent custody unit.
                let _g3 = GilGuard::new();
                let depth3 = execution_tls().gil_depth.get();
                assert_eq!(depth3, depth2 + 1);
                assert!(gil_held());
            }
            // Dropping the deepest guard leaves the other two live.
            assert_eq!(execution_tls().gil_depth.get(), depth2);
            assert!(gil_held());
        }
        // Dropping inner guards leaves the outer gate untouched.
        let depth1_after = execution_tls().gil_depth.get();
        assert_eq!(depth1_after, start + 1);
        assert!(gil_held());
    }

    // The outer guard's drop performed the 1->0 transition and released the
    // mutex exactly once.
    let final_depth = execution_tls().gil_depth.get();
    assert_eq!(final_depth, start, "depth restored after all guards drop");
    assert!(
        !gil_held(),
        "outer guard must release current-thread custody"
    );
}

#[test]
fn dropping_outer_guard_first_preserves_custody_until_last_guard() {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (proceed_tx, proceed_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        proceed_rx.recv().unwrap();
        let _guard = GilGuard::new();
        acquired_tx.send(()).unwrap();
    });
    ready_rx.recv().unwrap();
    let outer = GilGuard::new();
    let inner = GilGuard::new();
    assert_eq!(execution_tls().gil_depth.get(), 2);
    proceed_tx.send(()).unwrap();
    drop(outer);
    assert_eq!(execution_tls().gil_depth.get(), 1);
    assert!(gil_held());
    assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
    drop(inner);
    assert_eq!(execution_tls().gil_depth.get(), 0);
    assert!(!gil_held());
    acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
}

#[test]
fn releasing_persistent_custody_preserves_live_shutdown_guard() {
    super::hold_runtime_gil(GilGuard::new());
    let shutdown_guard = GilGuard::new();
    assert_eq!(execution_tls().gil_depth.get(), 2);

    super::release_runtime_gil();
    assert_eq!(execution_tls().gil_depth.get(), 1);
    assert!(gil_held());
    assert!(
        super::molt_gil().try_lock().is_none(),
        "dropping persistent custody must not unlock beneath shutdown"
    );

    drop(shutdown_guard);
    assert!(!gil_held());
}

struct LateGilProbe {
    suspended: mpsc::Sender<()>,
    observed: mpsc::Receiver<()>,
    completed: mpsc::Sender<()>,
}

impl Drop for LateGilProbe {
    fn drop(&mut self) {
        assert!(execution_tls().native_gil.lifetime_retired.get());
        assert!(molt_cpython_abi::api::object::runtime_thread_state_lifetime_is_retired());
        assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        {
            let execution = crate::concurrency::RuntimeExecutionGuard::enter();
            assert!(
                !unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null()
            );
            let release = super::GilReleaseGuard::suspend();
            drop(release);
            assert!(execution.token().execution_tls().gil_depth.get() > 0);
        }
        assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        let outer = GilGuard::new();
        let inner = GilGuard::new();
        assert_eq!(execution_tls().gil_depth.get(), 2);
        let release = super::GilReleaseGuard::suspend();
        assert!(!gil_held());
        self.suspended.send(()).unwrap();
        self.observed.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(release);
        assert_eq!(execution_tls().gil_depth.get(), 2);
        drop(outer);
        assert!(gil_held());
        drop(inner);
        assert!(!gil_held());
        assert_eq!(execution_tls().native_gil.suspension_identity(), 0);
        self.completed.send(()).unwrap();
    }
}

#[test]
fn a_real_tls_destructor_suspends_and_restores_ordinary_gil_custody() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    thread_local! {
        static LATE: std::cell::RefCell<Option<LateGilProbe>> = const {
            std::cell::RefCell::new(None)
        };
    }
    let (suspended_tx, suspended_rx) = mpsc::channel();
    let (observed_tx, observed_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let observer = std::thread::spawn(move || {
        suspended_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let _gil = GilGuard::new();
        observed_tx.send(()).unwrap();
    });
    let worker = std::thread::spawn(move || {
        // Initialized first, LATE drops after the actual GIL lifetime
        // sentinel. No destructor-bearing GIL TLS key can serve this probe.
        LATE.with(|slot| {
            *slot.borrow_mut() = Some(LateGilProbe {
                suspended: suspended_tx,
                observed: observed_rx,
                completed: completed_tx,
            });
        });
        let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    });
    completed_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    worker.join().unwrap();
    observer.join().unwrap();
}

#[test]
fn nested_suspensions_preserve_the_dormant_owner_and_unwind() {
    let unwind = std::panic::catch_unwind(|| {
        let _outer = GilGuard::new();
        let _suspended = super::GilReleaseGuard::suspend();
        {
            let _callback = GilGuard::new();
            let _nested = super::GilReleaseGuard::suspend();
        }
        panic!("nested saved-GIL unwind witness");
    });
    assert!(unwind.is_err());
    assert!(!gil_held());
    assert_eq!(execution_tls().native_gil.suspension_identity(), 0);
    let (completed_tx, completed_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        drop(GilGuard::new());
        completed_tx.send(()).unwrap();
    });
    completed_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
}

#[test]
fn gil_release_guard_drops_runtime_lock_temporarily() {
    super::hold_runtime_gil(GilGuard::new());
    let release = super::GilReleaseGuard::suspend();

    let acquired = Arc::new(AtomicBool::new(false));
    let acquired_flag = Arc::clone(&acquired);
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(lock) = super::molt_gil().try_lock() {
                acquired_flag.store(true, Ordering::SeqCst);
                drop(lock);
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    worker.join().expect("worker should not panic");
    assert!(
        acquired.load(Ordering::SeqCst),
        "runtime GIL lock should be available while GilReleaseGuard is active",
    );

    let encoded = release.into_encoded_state();
    // SAFETY: this is the unmatched token produced above, consumed on the
    // same thread exactly once.
    drop(unsafe { super::GilReleaseGuard::from_encoded_state(encoded) });
    super::release_runtime_gil();
}

#[test]
fn gil_release_without_current_thread_custody_fails_closed() {
    assert!(!gil_held());
    assert!(
        crate::test_support::catch_expected_unwind(super::GilReleaseGuard::suspend).is_err(),
        "release without current-thread GIL custody must not encode a no-op"
    );
    assert!(!gil_held());
}

#[test]
fn gil_ownership_is_thread_local_and_released() {
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let before = super::gil_owned_by_current_thread();
        let during = {
            let _gil = GilGuard::new();
            super::gil_owned_by_current_thread()
        };
        let after = super::gil_owned_by_current_thread();
        tx.send((before, during, after))
            .expect("worker should report ownership transitions");
    });
    let (before, during, after) = rx.recv().expect("main should receive ownership state");
    assert!(!before, "new worker must not inherit GIL ownership");
    assert!(during, "worker guard must establish GIL ownership");
    assert!(!after, "dropping the outer guard must release ownership");
    worker.join().expect("worker should not panic");
}
