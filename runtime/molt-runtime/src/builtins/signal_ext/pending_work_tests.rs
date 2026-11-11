use super::*;
use crate::state::pending_work::PendingWorkKind;

#[test]
fn route_publication_adopts_pending_calls_whose_notification_saw_no_runtime() {
    static CALLED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn callback(_: *mut std::ffi::c_void) -> i32 {
        CALLED.fetch_add(1, Ordering::Relaxed);
        0
    }
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        crate::with_gil_entry_nopanic!(py, {
            CALLED.store(0, Ordering::Relaxed);
            let state = runtime_state(py);
            assert!(signal_runtime_state_deactivate(&state.signal));
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
            let word = unsafe { &*state.pending_work.word_ptr() };
            assert_eq!(
                word.load(Ordering::Relaxed) & PendingWorkKind::PendingCalls as u32,
                0
            );
            assert!(signal_runtime_state_publish(state));
            assert_ne!(
                word.load(Ordering::Relaxed) & PendingWorkKind::PendingCalls as u32,
                0
            );
            crate::builtins::exceptions::molt_async_work_poll_and_exception_pending_attached_body(
                py,
            );
            assert_eq!(CALLED.load(Ordering::Relaxed), 1);
        });
    });
}
