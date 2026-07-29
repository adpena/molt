//! Shared process-wide test authorities.
//!
//! Expected panics must not invoke the platform backtrace resolver. On Windows,
//! many concurrent, deliberately caught panics can otherwise deadlock inside
//! `dbghelp` while test and worker threads are entering loader/TLS teardown.
//! Unexpected panics still delegate to the original hook unchanged.

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, MutexGuard, Once};

thread_local! {
    static EXPECTED_PANIC_DEPTH: Cell<u32> = const { Cell::new(0) };
}

static INSTALL_EXPECTED_PANIC_HOOK: Once = Once::new();
static PROCESS_GLOBAL_TEST_STATE: Mutex<()> = Mutex::new(());

struct RuntimeTestRestartCustody {
    owner: std::thread::ThreadId,
    active: bool,
}

impl RuntimeTestRestartCustody {
    fn enter() -> Self {
        let owner = std::thread::current().id();
        crate::state::runtime_state::begin_runtime_test_restart(owner);
        Self {
            owner,
            active: true,
        }
    }

    fn finish(&mut self) {
        if self.active {
            crate::state::runtime_state::end_runtime_test_restart(self.owner);
            self.active = false;
        }
    }
}

impl Drop for RuntimeTestRestartCustody {
    fn drop(&mut self) {
        self.finish();
    }
}

struct PendingCallTestCustody {
    snapshot: Option<molt_cpython_abi::api::pending_calls::PendingCallRuntimeTestSnapshot>,
}

impl PendingCallTestCustody {
    fn enter() -> Self {
        Self {
            snapshot: Some(
                molt_cpython_abi::api::pending_calls::begin_runtime_test_transaction(
                    std::thread::current().id(),
                ),
            ),
        }
    }

    fn restore(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            crate::with_gil_entry_nopanic!(_py, {
                molt_cpython_abi::api::pending_calls::restore_runtime_test_transaction(snapshot);
            });
        }
    }

    fn reset(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            molt_cpython_abi::api::pending_calls::reset_runtime_test_transaction(snapshot);
        }
    }
}

impl Drop for PendingCallTestCustody {
    fn drop(&mut self) {
        self.restore();
    }
}

fn process_global_test_state() -> MutexGuard<'static, ()> {
    PROCESS_GLOBAL_TEST_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct TrustedTestEnvironment(Option<std::ffi::OsString>);

impl TrustedTestEnvironment {
    fn enter() -> Self {
        let prior = std::env::var_os("MOLT_TRUSTED");
        unsafe { std::env::set_var("MOLT_TRUSTED", "1") };
        Self(prior)
    }
}

impl Drop for TrustedTestEnvironment {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => unsafe { std::env::set_var("MOLT_TRUSTED", value) },
            None => unsafe { std::env::remove_var("MOLT_TRUSTED") },
        }
    }
}

struct PendingExceptionSnapshot {
    c_error: Option<molt_cpython_abi::api::errors::OwnedCError>,
    runtime_error_bits: Option<u64>,
}

impl PendingExceptionSnapshot {
    fn detach() -> Self {
        let c_error = molt_cpython_abi::api::errors::take_current_error();
        let runtime_error_bits = crate::with_gil_entry_nopanic!(_py, {
            if !crate::exception_pending(_py) {
                None
            } else {
                let bits = crate::exception_last_bits_noinc(_py)
                    .expect("pending runtime exception must have an owned instance");
                crate::inc_ref_bits(_py, bits);
                crate::clear_exception(_py);
                Some(bits)
            }
        });
        Self {
            c_error,
            runtime_error_bits,
        }
    }

    fn restore(self) {
        drop(molt_cpython_abi::api::errors::take_current_error());
        crate::with_gil_entry_nopanic!(_py, {
            crate::clear_exception(_py);
            if let Some(bits) = self.runtime_error_bits {
                let ptr = crate::obj_from_bits(bits)
                    .as_ptr()
                    .expect("snapshotted runtime exception must remain live");
                crate::record_exception(_py, ptr);
                crate::dec_ref_bits(_py, bits);
            }
        });
        if let Some(error) = self.c_error {
            molt_cpython_abi::api::errors::restore_current_error_exact(error);
        }
    }
}

/// One scoped authority for process-global runtime test state.
///
/// Construction explicitly initializes the runtime, installs the CPython ABI
/// hooks through that production bootstrap, borrows pending-call main-thread
/// custody for the current harness thread, and detaches both exception domains.
/// Drop restores the exact borrowed state even after an expected test panic.
pub(crate) struct RuntimeTestTransaction {
    pending_calls: PendingCallTestCustody,
    pending_exceptions: Option<PendingExceptionSnapshot>,
    gc: Option<crate::object::gc::GcRuntimeTestSnapshot>,
    execution_thread_attached: bool,
    _process_state: MutexGuard<'static, ()>,
}

impl RuntimeTestTransaction {
    pub(crate) fn new() -> Self {
        Self::enter(false)
    }

    pub(crate) fn with_gc_isolation() -> Self {
        Self::enter(true)
    }

    /// Run one test against a freshly bootstrapped trusted runtime.
    ///
    /// Import-boundary tests need their environment frozen during cold
    /// bootstrap, so they cannot enter the normal already-ready transaction.
    /// This is the sole test authority for the shutdown/reset/reinitialize
    /// lifecycle: it owns process-state custody, restores the environment, and
    /// leaves the lifecycle reset even when the test body unwinds.
    pub(crate) fn with_trusted_fresh_runtime<R>(f: impl FnOnce() -> R) -> R {
        let _process_state = process_global_test_state();
        let mut restart = RuntimeTestRestartCustody::enter();
        let _trusted_environment = TrustedTestEnvironment::enter();
        let mut pending_calls = PendingCallTestCustody::enter();

        if crate::state::runtime_state::runtime_is_initialized() {
            assert_eq!(
                crate::state::runtime_state::molt_runtime_shutdown(),
                1,
                "fresh runtime transaction could not retire the prior runtime"
            );
        }
        crate::state::runtime_state::molt_runtime_reset_for_testing();
        assert_eq!(
            crate::state::runtime_state::molt_runtime_init(),
            1,
            "fresh runtime transaction could not bootstrap"
        );

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(f));
        if crate::state::runtime_state::runtime_is_initialized() {
            assert_eq!(
                crate::state::runtime_state::molt_runtime_shutdown(),
                1,
                "fresh runtime transaction could not retire its runtime"
            );
        }
        crate::state::runtime_state::molt_runtime_reset_for_testing();
        pending_calls.reset();
        restart.finish();

        match outcome {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn enter(isolate_gc: bool) -> Self {
        let process_state = process_global_test_state();
        let pending_calls = PendingCallTestCustody::enter();
        assert_eq!(
            crate::state::runtime_state::molt_runtime_init(),
            1,
            "runtime test transaction requires successful production bootstrap"
        );
        let execution_thread_attached =
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached();
        let pending_exceptions = PendingExceptionSnapshot::detach();
        let gc = isolate_gc.then(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let state = &crate::runtime_state(_py).gc;
                let snapshot = state.runtime_test_snapshot();
                let outcome = unsafe { crate::object::gc::collect_cycles(_py) };
                assert!(
                    matches!(
                        outcome.status,
                        crate::object::gc::GcCollectStatus::Completed
                            | crate::object::gc::GcCollectStatus::ReentrantNoop
                            | crate::object::gc::GcCollectStatus::UnsupportedConcurrency
                    ),
                    "runtime test GC baseline failed: {:?}",
                    outcome.status
                );
                state.restore_runtime_test_snapshot(&snapshot);
                snapshot
            })
        });
        Self {
            pending_calls,
            pending_exceptions: Some(pending_exceptions),
            gc,
            execution_thread_attached,
            _process_state: process_state,
        }
    }
}

impl Drop for RuntimeTestTransaction {
    fn drop(&mut self) {
        if let Some(snapshot) = self.gc.take() {
            crate::with_gil_entry_nopanic!(_py, {
                let outcome = unsafe { crate::object::gc::collect_cycles(_py) };
                if !std::thread::panicking() {
                    assert!(
                        matches!(
                            outcome.status,
                            crate::object::gc::GcCollectStatus::Completed
                                | crate::object::gc::GcCollectStatus::ReentrantNoop
                                | crate::object::gc::GcCollectStatus::UnsupportedConcurrency
                        ),
                        "runtime test GC cleanup failed: {:?}",
                        outcome.status
                    );
                }
                crate::runtime_state(_py)
                    .gc
                    .restore_runtime_test_snapshot(&snapshot);
            });
        }
        if let Some(snapshot) = self.pending_exceptions.take() {
            snapshot.restore();
        }
        self.pending_calls.restore();
        if !std::thread::panicking() {
            assert_eq!(
                molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
                self.execution_thread_attached,
                "current-thread runtime execution attachment leaked across test transaction"
            );
        }
    }
}

fn install_expected_panic_hook() {
    INSTALL_EXPECTED_PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let expected = EXPECTED_PANIC_DEPTH
                .try_with(|depth| depth.get() != 0)
                .unwrap_or(false);
            if !expected {
                previous(info);
            }
        }));
    });
}

struct ExpectedPanicGuard;

impl ExpectedPanicGuard {
    fn enter() -> Self {
        install_expected_panic_hook();
        EXPECTED_PANIC_DEPTH.with(|depth| {
            depth.set(
                depth
                    .get()
                    .checked_add(1)
                    .expect("expected-panic nesting overflow"),
            );
        });
        Self
    }
}

impl Drop for ExpectedPanicGuard {
    fn drop(&mut self) {
        EXPECTED_PANIC_DEPTH.with(|depth| {
            let current = depth.get();
            assert_ne!(current, 0, "unmatched expected-panic guard");
            depth.set(current - 1);
        });
    }
}

pub(crate) fn with_expected_panic<F, R>(operation: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = ExpectedPanicGuard::enter();
    operation()
}

pub(crate) fn catch_expected_unwind<F, R>(operation: F) -> std::thread::Result<R>
where
    F: FnOnce() -> R,
{
    with_expected_panic(|| std::panic::catch_unwind(AssertUnwindSafe(operation)))
}

#[test]
fn expected_panic_hook_is_thread_local_and_nestable() {
    assert!(catch_expected_unwind(|| panic!("outer expected panic")).is_err());
    assert!(
        catch_expected_unwind(|| {
            assert!(catch_expected_unwind(|| panic!("inner expected panic")).is_err());
            panic!("second outer expected panic");
        })
        .is_err()
    );
}

#[test]
#[ignore = "runtime test transaction latency/allocation probe"]
fn runtime_test_transaction_overhead_probe() {
    const ITERATIONS: u32 = 4_096;
    drop(RuntimeTestTransaction::new());
    #[cfg(feature = "l7-attestation-probe")]
    {
        crate::attestation_probe::reset();
        crate::attestation_probe::set_tracking(true);
    }
    let started = std::time::Instant::now();
    for _ in 0..ITERATIONS {
        drop(RuntimeTestTransaction::new());
    }
    let elapsed = started.elapsed();
    #[cfg(feature = "l7-attestation-probe")]
    {
        crate::attestation_probe::set_tracking(false);
        let allocation = crate::attestation_probe::snapshot();
        assert_eq!(
            allocation.allocations, 0,
            "warm runtime test transactions must remain allocation-free"
        );
    }
    println!(
        "{{\"iterations\":{ITERATIONS},\"elapsed_ns\":{},\"ns_per_transaction\":{}}}",
        elapsed.as_nanos(),
        elapsed.as_nanos() / u128::from(ITERATIONS),
    );
}
