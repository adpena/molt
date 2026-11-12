use super::*;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Barrier};

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn publication_during_service_survives_clear_and_preserves_other_sources() {
    let word = Arc::new(PendingWork::new());
    let source = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let published = Arc::new(Barrier::new(2));
    word.request(PendingWorkKind::Signals);
    word.request(PendingWorkKind::PendingCalls);
    let producer = {
        let (word, source, published) = (word.clone(), source.clone(), published.clone());
        std::thread::spawn(move || {
            source.store(true, Ordering::Release);
            word.request(PendingWorkKind::PendingCalls);
            published.wait();
        })
    };
    published.wait();
    // Service completed the *old* callback, but a producer published another
    // before its summary bit was cleared. The queue recheck must retain it.
    word.serviced(PendingWorkKind::PendingCalls, || {
        source.load(Ordering::Acquire)
    });
    producer.join().unwrap();
    assert_eq!(word.0.load(Ordering::Relaxed), 9);
    source.store(false, Ordering::Release);
    word.serviced(PendingWorkKind::PendingCalls, || {
        source.load(Ordering::Acquire)
    });
    assert_eq!(word.0.load(Ordering::Relaxed), 1);
    word.serviced(PendingWorkKind::Signals, || false);
    assert_eq!(word.0.load(Ordering::Relaxed), 0);
}

#[test]
fn take_reports_and_clears_only_its_own_request() {
    let word = PendingWork::new();
    assert!(!word.take(PendingWorkKind::GilDropRequest));
    word.request(PendingWorkKind::Signals);
    word.request(PendingWorkKind::GilDropRequest);
    assert!(word.take(PendingWorkKind::GilDropRequest));
    assert!(!word.take(PendingWorkKind::GilDropRequest));
    assert_eq!(
        word.0.load(Ordering::Relaxed),
        PendingWorkKind::Signals as u32
    );
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn c_thread_publication_reaches_inline_poll_and_omitted_notification_starves_it() {
    use std::sync::atomic::AtomicUsize;
    static CALLBACKS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn callback(_: *mut std::ffi::c_void) -> i32 {
        CALLBACKS.fetch_add(1, Ordering::Relaxed);
        0
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        CALLBACKS.store(0, Ordering::Relaxed);
        let state = crate::runtime_state(py);
        // Observe exactly the word the emitted code receives, not an unrelated
        // test flag, and use the real C-API publisher on a thread without a GIL.
        let record = (unsafe {
            crate::PyToken::from_attached_context(crate::state::molt_attached_context())
        }
        .execution_tls() as *const crate::state::ExecutionTls as u64)
            as *const u8;
        let ptr = unsafe {
            *(record.add(molt_codegen_abi::THREAD_RECORD_PENDING_WORK_PTR_OFFSET as usize)
                as *const u64)
        };
        assert_eq!(ptr, state.pending_work.word_ptr() as u64);
        let word = unsafe { &*(ptr as *const AtomicU32) };
        crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(py);
        assert_eq!(
            word.load(Ordering::Relaxed),
            0,
            "test requires an empty runtime"
        );
        let enqueue = || {
            assert_eq!(
                std::thread::spawn(|| unsafe {
                    molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                        Some(callback),
                        std::ptr::null_mut(),
                    )
                })
                .join()
                .unwrap(),
                0
            );
        };
        enqueue();
        assert_ne!(
            word.load(Ordering::Relaxed) & PendingWorkKind::PendingCalls as u32,
            0
        );
        let origin = crate::state::runtime_state::RuntimeWorkerOrigin::new(state);
        let worker = {
            let _released = crate::GilReleaseGuard::suspend();
            std::thread::spawn(move || {
                crate::state::run_runtime_worker(origin, || {
                    crate::with_gil_entry_nopanic!(_worker, {
                        let pending =
                            crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(_worker
                            );
                        (pending, CALLBACKS.load(Ordering::Relaxed))
                    })
                })
            })
            .join()
            .unwrap()
        };
        assert_eq!(
            worker,
            (0, 0),
            "a worker cannot dispatch the main thread's C call"
        );
        assert_ne!(
            word.load(Ordering::Relaxed) & PendingWorkKind::PendingCalls as u32,
            0,
            "a worker cannot clear main-thread-only work"
        );
        if word.load(Ordering::Relaxed) != 0 {
            assert_eq!(
                crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(py),
                0
            );
        }
        assert_eq!(CALLBACKS.load(Ordering::Relaxed), 1);
        assert_eq!(
            word.load(Ordering::Relaxed) & PendingWorkKind::PendingCalls as u32,
            0
        );

        enqueue();
        // Mutation control: remove just the producer's notification. Real
        // queued work remains, but the inline word gate cannot discover it.
        word.fetch_and(!(PendingWorkKind::PendingCalls as u32), Ordering::AcqRel);
        if word.load(Ordering::Relaxed) != 0 {
            crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(
                py,
            );
        }
        assert!(molt_cpython_abi::api::pending_calls::has_pending_calls());
        assert_eq!(CALLBACKS.load(Ordering::Relaxed), 1);
        state.pending_work.request(PendingWorkKind::PendingCalls);
        crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(py);
        assert_eq!(CALLBACKS.load(Ordering::Relaxed), 2);
    });
}

#[test]
fn interrupt_publication_reaches_inline_gate_before_exception_is_raised() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let state = crate::runtime_state(py);
        assert_eq!(
            unsafe { molt_cpython_abi::api::errors::PyErr_SetInterruptEx(libc::SIGINT) },
            0
        );
        assert!(!crate::exception_pending(py));
        assert_ne!(
            state.pending_work.0.load(Ordering::Relaxed) & PendingWorkKind::Signals as u32,
            0
        );
        assert_eq!(
            crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(
                py
            ),
            1
        );
        assert_eq!(
            crate::builtins::exceptions::pending_exception_diagnostic(py)
                .unwrap()
                .0,
            "KeyboardInterrupt"
        );
        crate::builtins::exceptions::molt_exception_clear_attached_body(py);
        assert_eq!(
            state.pending_work.0.load(Ordering::Relaxed) & PendingWorkKind::Signals as u32,
            0
        );
    });
}

#[test]
#[cfg(not(feature = "free-threaded"))]
fn allocation_pressure_notifies_the_inline_gate_and_service_collects_it() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let state = crate::runtime_state(py);
        state.gc.set_enabled(false);
        unsafe { crate::object::gc::collect_cycles(py) };
        state.gc.set_thresholds([2, 10, 10]);
        state.gc.set_enabled(true);
        let before = state.gc.generation_stats()[0].collections;
        let roots: Vec<_> = (0..3).map(|_| crate::alloc_list(py, &[])).collect();
        assert_eq!(state.gc.generation_stats()[0].collections, before);
        assert_ne!(
            state.pending_work.0.load(Ordering::Relaxed)
                & PendingWorkKind::GarbageCollection as u32,
            0
        );
        crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(py);
        assert_eq!(state.gc.generation_stats()[0].collections, before + 1);
        assert_eq!(
            state.pending_work.0.load(Ordering::Relaxed)
                & PendingWorkKind::GarbageCollection as u32,
            0
        );
        for ptr in roots {
            crate::dec_ref_bits(py, crate::MoltObject::from_ptr(ptr).bits());
        }
    });
}
