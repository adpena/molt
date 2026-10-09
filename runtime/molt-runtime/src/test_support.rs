//! Shared process-wide test authorities.
//!
//! Expected panics must not invoke the platform backtrace resolver. On Windows,
//! many concurrent, deliberately caught panics can otherwise deadlock inside
//! `dbghelp` while test and worker threads are entering loader/TLS teardown.
//! Unexpected panics still delegate to the original hook unchanged.

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, MutexGuard, Once};

use molt_runtime_core::host_capabilities_generated::MAXIMUM_BUILTIN_CAPABILITY_TIER;

#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

pub(crate) mod captured_runtime_children {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/captured_runtime_children.rs"
    ));
}

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
    restore_prior_runtime: bool,
    snapshot: Option<molt_cpython_abi::api::pending_calls::PendingCallRuntimeTestSnapshot>,
}

impl PendingCallTestCustody {
    fn enter() -> Self {
        Self {
            restore_prior_runtime: true,
            snapshot: Some(
                molt_cpython_abi::api::pending_calls::begin_runtime_test_transaction(
                    std::thread::current().id(),
                ),
            ),
        }
    }

    fn restore(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            // Serialize the one queue consumer without entering or initializing
            // RuntimeState. Finish only quiesces producers and discards opaque C
            // callback tokens; it never invokes them or releases Python owners.
            let _gil = crate::concurrency::GilGuard::new();
            if self.restore_prior_runtime && crate::state::runtime_state::runtime_is_ready() {
                molt_cpython_abi::api::pending_calls::restore_runtime_test_transaction(snapshot);
            } else {
                molt_cpython_abi::api::pending_calls::reset_runtime_test_transaction(snapshot);
            }
        }
    }

    fn prior_runtime_retired(&mut self) {
        // A destructive transaction must never restore an earlier generation's
        // queue owner if bootstrap, the body, or final cleanup subsequently fails.
        self.restore_prior_runtime = false;
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
    #[cfg(not(target_arch = "wasm32"))]
    assert!(
        !crate::concurrency::gil_held(),
        "runtime test transaction must acquire process-state custody before the GIL"
    );
    PROCESS_GLOBAL_TEST_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct MaximumCapabilityTierTestEnvironment(Option<std::ffi::OsString>);

impl MaximumCapabilityTierTestEnvironment {
    fn enter() -> Self {
        let prior = std::env::var_os("MOLT_CAPABILITY_TIER");
        unsafe { std::env::set_var("MOLT_CAPABILITY_TIER", MAXIMUM_BUILTIN_CAPABILITY_TIER) };
        Self(prior)
    }
}

impl Drop for MaximumCapabilityTierTestEnvironment {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => unsafe { std::env::set_var("MOLT_CAPABILITY_TIER", value) },
            None => unsafe { std::env::remove_var("MOLT_CAPABILITY_TIER") },
        }
    }
}

struct PendingExceptionSnapshot {
    c_error: Option<molt_cpython_abi::api::errors::OwnedCError>,
    runtime_error: Option<crate::builtins::exceptions::RaisedSnapshot>,
}

impl PendingExceptionSnapshot {
    fn detach() -> Self {
        let c_error = molt_cpython_abi::api::errors::take_current_error();
        let runtime_error = crate::with_gil_entry_nopanic!(py, {
            Some(crate::builtins::exceptions::take_raised(py))
        });
        Self {
            c_error,
            runtime_error,
        }
    }

    fn restore(mut self) {
        if !crate::state::runtime_state::runtime_is_ready() {
            // Failed lifecycle allocations remain pinned until process exit.
            // OwnedCError::drop would decref into their partially retired graph.
            std::mem::forget(self);
            assert!(
                std::thread::panicking(),
                "test exception restoration requires a live runtime"
            );
            return;
        }
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        if let Some(error) = self.c_error {
            molt_cpython_abi::api::errors::restore_current_error_exact(error);
        }
        crate::with_gil_entry_nopanic!(py, {
            crate::builtins::exceptions::resolve_raised(py, &mut self.runtime_error);
        });
    }
}

#[test]
fn pending_exception_snapshot_preserves_emergency_runtime_state() {
    use crate::builtins::exceptions::RaisedSnapshot;

    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        crate::record_memory_error_without_allocation(py);
        let snapshot = PendingExceptionSnapshot::detach();
        assert!(matches!(
            snapshot.runtime_error,
            Some(RaisedSnapshot::Emergency(_))
        ));
        assert!(!crate::exception_pending(py));
        crate::record_memory_error_without_allocation(py);
        snapshot.restore();
        assert!(crate::exception_pending(py));
        let restored = PendingExceptionSnapshot::detach();
        assert!(matches!(
            restored.runtime_error,
            Some(RaisedSnapshot::Emergency(_))
        ));
        restored.restore();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    });
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
    interpreter_sys: Option<crate::builtins::module_table::InterpreterSysTestSnapshot>,
    gc: Option<crate::object::gc::GcRuntimeTestSnapshot>,
    execution_thread_attached: bool,
    retained_thread_state_before: bool,
    _process_state: MutexGuard<'static, ()>,
}

/// Private restoration for one synchronous target operation. Callers cannot
/// retain or reorder guards; nested operations restore in lexical order.
struct RuntimeTargetPython<'transaction, 'token, 'gil> {
    _transaction: &'transaction RuntimeTestTransaction,
    py: &'token crate::PyToken<'gil>,
    prior: Option<crate::state::runtime_state::PythonVersionInfo>,
}

impl Drop for RuntimeTargetPython<'_, '_, '_> {
    fn drop(&mut self) {
        let mut target = crate::runtime_state(self.py)
            .sys_version_info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *target = self.prior.take();
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RuntimeTestLifecycleMode {
    TrustedFresh,
    Cold,
}

impl RuntimeTestTransaction {
    pub(crate) fn new() -> Self {
        Self::enter(false)
    }

    pub(crate) fn with_gc_isolation() -> Self {
        Self::enter(true)
    }

    /// Borrow the runtime's exact target for one synchronous operation.
    ///
    /// The storage lock is released before the body, including nested GIL
    /// entries. Private restoration preserves both normal results and the
    /// original panic payload without initializing a missing prior target.
    pub(crate) fn with_target_python<R>(
        &self,
        py: &crate::PyToken<'_>,
        target: Option<crate::state::runtime_state::PythonVersionInfo>,
        operation: impl FnOnce() -> R,
    ) -> R {
        let prior = std::mem::replace(
            &mut *crate::runtime_state(py)
                .sys_version_info
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            target,
        );
        let _target = RuntimeTargetPython {
            _transaction: self,
            py,
            prior,
        };
        operation()
    }

    /// The shared fixture for versioned Python 3 minor semantics.
    pub(crate) fn with_target_python_minor<R>(
        &self,
        py: &crate::PyToken<'_>,
        minor: i64,
        operation: impl FnOnce() -> R,
    ) -> R {
        self.with_target_python(
            py,
            Some(crate::state::runtime_state::PythonVersionInfo {
                major: 3,
                minor,
                micro: 0,
                releaselevel: "final".to_string(),
                serial: 0,
            }),
            operation,
        )
    }

    /// Run one test against a freshly bootstrapped trusted runtime.
    ///
    /// Import-boundary tests need their environment frozen during cold
    /// bootstrap, so they cannot enter the normal already-ready transaction.
    /// This delegates to the shared shutdown/reset lifecycle authority while
    /// adding restart custody, the trusted environment, and eager bootstrap.
    pub(crate) fn with_trusted_fresh_runtime<R>(f: impl FnOnce() -> R) -> R {
        Self::with_runtime_lifecycle(RuntimeTestLifecycleMode::TrustedFresh, f)
    }

    /// Run a test that owns the complete cold runtime lifecycle.
    ///
    /// The body starts with an uninitialized runtime and chooses which thread
    /// performs production initialization and shutdown. Unlike the ordinary
    /// transaction, this destructive lifecycle authority does not snapshot
    /// exception state, establish an incidental CPython thread-state record,
    /// force trusted capabilities, or block worker initialization behind test
    /// restart custody. Cleanup still retires any runtime left Ready, resets the
    /// one-shot lifecycle state, and leaves pending-call custody unowned.
    pub(crate) fn with_cold_runtime_lifecycle<R>(f: impl FnOnce() -> R) -> R {
        Self::with_runtime_lifecycle(RuntimeTestLifecycleMode::Cold, f)
    }

    fn with_runtime_lifecycle<R>(mode: RuntimeTestLifecycleMode, f: impl FnOnce() -> R) -> R {
        let _process_state = process_global_test_state();
        assert!(
            crate::state::runtime_state::runtime_execution_is_admitted_for_current_thread(false),
            "runtime lifecycle transaction requires a restartable runtime"
        );
        let mut restart =
            (mode == RuntimeTestLifecycleMode::TrustedFresh).then(RuntimeTestRestartCustody::enter);
        let _capability_environment = (mode == RuntimeTestLifecycleMode::TrustedFresh)
            .then(MaximumCapabilityTierTestEnvironment::enter);
        let mut pending_calls = PendingCallTestCustody::enter();

        if crate::state::runtime_state::runtime_is_initialized() {
            assert_eq!(
                crate::state::runtime_state::molt_runtime_shutdown(),
                1,
                "runtime lifecycle transaction could not retire the prior runtime"
            );
        }
        pending_calls.prior_runtime_retired();
        crate::state::runtime_state::molt_runtime_reset_for_testing();
        if mode == RuntimeTestLifecycleMode::Cold {
            // The prior runtime's pending-call owner was retired with it. Do
            // not bind the cold body's initializer to this harness thread: the
            // production initialization winner must select the new owner.
            pending_calls.reset();
            #[cfg(not(target_arch = "wasm32"))]
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                0,
                "cold runtime lifecycle started with an incidental CPython thread-state owner"
            );
        } else {
            assert_eq!(
                crate::state::runtime_state::molt_runtime_init(),
                1,
                "fresh runtime transaction could not bootstrap"
            );
        }

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(f));
        let cleanup = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if crate::state::runtime_state::runtime_is_initialized() {
                assert_eq!(
                    crate::state::runtime_state::molt_runtime_shutdown(),
                    1,
                    "runtime lifecycle transaction could not retire its runtime"
                );
            }
            crate::state::runtime_state::molt_runtime_reset_for_testing();
            if mode == RuntimeTestLifecycleMode::Cold {
                // Production initialization selected the body's real main-thread
                // owner after the entry snapshot was reset. Borrow that completed
                // lifecycle once, prove its ring is empty, and clear the retired
                // owner instead of leaking it into the next test runtime.
                let mut completed_pending_calls = PendingCallTestCustody::enter();
                completed_pending_calls.reset();
            } else {
                pending_calls.reset();
            }
            if let Some(restart) = restart.as_mut() {
                restart.finish();
            }
        }));
        match (outcome, cleanup) {
            (Ok(value), Ok(())) => value,
            (Ok(_), Err(cleanup)) => std::panic::resume_unwind(cleanup),
            (Err(primary), Ok(())) => std::panic::resume_unwind(primary),
            (Err(primary), Err(cleanup)) => {
                // Do not replace the body's original panic with a cleanup
                // assertion. Keep both diagnostics, including under a caught
                // panic hook, and leave a failed runtime permanently closed.
                use std::io::Write;
                let message = cleanup
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| cleanup.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string cleanup panic");
                let _ = writeln!(
                    std::io::stderr(),
                    "runtime test cleanup also failed: {message}"
                );
                std::panic::resume_unwind(primary)
            }
        }
    }

    fn enter(isolate_gc: bool) -> Self {
        let process_state = process_global_test_state();
        let retained_thread_state_before =
            molt_cpython_abi::api::object::current_thread_has_retained_runtime_state();
        assert_eq!(
            crate::state::runtime_state::molt_runtime_init(),
            1,
            "runtime test transaction requires successful production bootstrap"
        );
        // Failed bootstrap must not reopen the process-static pending queue.
        let pending_calls = PendingCallTestCustody::enter();
        let execution_thread_attached =
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached();
        let pending_exceptions = PendingExceptionSnapshot::detach();
        let interpreter_sys = crate::with_gil_entry_nopanic!(py, {
            crate::builtins::module_table::InterpreterSysTestSnapshot::detach(py)
        });
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
            interpreter_sys: Some(interpreter_sys),
            gc,
            execution_thread_attached,
            retained_thread_state_before,
            _process_state: process_state,
        }
    }
}

impl Drop for RuntimeTestTransaction {
    fn drop(&mut self) {
        if !crate::state::runtime_state::runtime_is_ready() {
            // There is no legal callback/decref boundary after terminal failure.
            // The saved C/runtime error and sys owners stay with the unrecoverable
            // runtime until process exit, as its production lifecycle requires.
            // This is bounded by this transaction; no new owner registry exists.
            std::mem::forget(self.pending_exceptions.take());
            drop(self.interpreter_sys.take());
            drop(self.gc.take());
            self.pending_calls.reset();
            assert!(
                std::thread::panicking(),
                "runtime test transaction lost its live runtime before restoration"
            );
            return;
        }
        if let Some(snapshot) = self.interpreter_sys.take() {
            crate::with_gil_entry_nopanic!(py, {
                snapshot.restore(py);
            });
        }
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
        // The harness thread can outlive this transaction. Retire only the
        // detached state this transaction created before releasing process-state
        // custody; otherwise its later TLS destructor races the next test's
        // global retained-owner assertions and destructive lifecycle reset.
        // A predecessor's record (including its restored exception) is borrowed.
        if !self.retained_thread_state_before
            && crate::state::runtime_state::runtime_is_initialized()
            && molt_cpython_abi::api::object::current_thread_has_retained_runtime_state()
        {
            drop(crate::concurrency::execution::RuntimeExecutionGuard::enter_with_worker_cleanup());
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
fn target_python_custody_restores_missing_and_complete_targets_after_unwind() {
    use crate::state::runtime_state::PythonVersionInfo;

    let transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let state = crate::runtime_state(py);
        let original = state.sys_version_info.lock().unwrap().clone();
        let prior = PythonVersionInfo {
            major: 3,
            minor: 13,
            micro: 7,
            releaselevel: "candidate".to_string(),
            serial: 2,
        };
        transaction.with_target_python(py, None, || {
            transaction.with_target_python(py, Some(prior.clone()), || {
                assert_eq!(crate::object::ops_sys::runtime_target_minor(py), 13);
                let marker = Box::new(7_u64);
                let marker_address = (&*marker as *const u64).addr();
                let returned = transaction.with_target_python_minor(py, 12, || {
                    assert_eq!(crate::object::ops_sys::runtime_target_minor(py), 12);
                    transaction.with_target_python_minor(py, 14, || {
                        crate::with_gil_entry_nopanic!(nested_py, {
                            assert_eq!(crate::object::ops_sys::runtime_target_minor(nested_py), 14);
                        });
                    });
                    if crate::object::ops_sys::runtime_target_minor(py) == 12 {
                        return marker;
                    }
                    panic!("nested target was not restored before early return");
                });
                assert_eq!((&*returned as *const u64).addr(), marker_address);
                assert_eq!(*returned, 7);
                assert!(state.sys_version_info.lock().unwrap().as_ref() == Some(&prior));

                let marker = Box::new(42_u64);
                let marker_address = (&*marker as *const u64).addr();
                let failure = catch_expected_unwind(|| -> () {
                    transaction.with_target_python_minor(py, 14, || {
                        assert_eq!(crate::object::ops_sys::runtime_target_minor(py), 14);
                        std::panic::resume_unwind(marker);
                    });
                })
                .expect_err("the inner panic must escape target custody");
                let restored_marker = failure.downcast::<u64>().expect("original panic payload");
                assert_eq!((&*restored_marker as *const u64).addr(), marker_address);
                assert_eq!(*restored_marker, 42);
                assert!(state.sys_version_info.lock().unwrap().as_ref() == Some(&prior));
            });
            assert!(state.sys_version_info.lock().unwrap().is_none());

            let marker = Box::new(99_u64);
            let marker_address = (&*marker as *const u64).addr();
            let failure = catch_expected_unwind(|| -> () {
                transaction.with_target_python(py, Some(prior.clone()), || {
                    transaction.with_target_python_minor(py, 12, || {
                        assert_eq!(crate::object::ops_sys::runtime_target_minor(py), 12);
                        std::panic::resume_unwind(marker);
                    });
                });
            })
            .expect_err("the original panic must escape all nested target operations");
            let restored_marker = failure.downcast::<u64>().expect("original panic payload");
            assert_eq!((&*restored_marker as *const u64).addr(), marker_address);
            assert_eq!(*restored_marker, 99);
            assert!(state.sys_version_info.lock().unwrap().is_none());
        });
        assert!(*state.sys_version_info.lock().unwrap() == original);
    });
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

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn transaction_retires_created_thread_state_before_harness_tls_exit() {
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
    let (exit_tx, exit_rx) = std::sync::mpsc::channel();
    let harness = std::thread::spawn(move || {
        assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        {
            let _transaction = RuntimeTestTransaction::new();
            // Snapshotting exception state crosses the real execution boundary.
            assert!(molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        }
        let retained = molt_cpython_abi::api::object::current_thread_has_retained_runtime_state();
        dropped_tx.send(retained).unwrap();
        // Keep native TLS alive after releasing the transaction's mutex.
        exit_rx.recv().unwrap();
    });
    let retained_after_drop = dropped_rx.recv().unwrap();
    exit_tx.send(()).unwrap();
    harness.join().unwrap();
    assert!(
        !retained_after_drop,
        "transaction-created owner survived until harness TLS exit"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn transaction_rejects_gil_before_process_state_lock() {
    let _gil = crate::concurrency::GilGuard::new();
    let failure = catch_expected_unwind(RuntimeTestTransaction::new);
    assert!(
        failure.is_err(),
        "GIL-first fixture admission must fail before waiting on the process-state mutex"
    );
}

/// Standalone runtime tests explicitly publish the same provider namespace as
/// a compiled initializer. This fixture owns/restores its cache projection;
/// production lookup has no test-only materialization or secondary cache.
pub(crate) struct NativeProviderTestNamespace {
    name: u64,
    previous: u64,
    module: u64,
}

impl NativeProviderTestNamespace {
    pub(crate) fn new(py: &crate::PyToken<'_>, provider: &str) -> Self {
        let name = crate::attr_name_bits_from_bytes(py, provider.as_bytes()).unwrap();
        let previous = crate::molt_module_cache_get(name);
        crate::clear_exception(py);
        crate::molt_module_cache_del(name);
        crate::clear_exception(py);
        let module = crate::molt_module_new(name);
        assert!(!crate::obj_from_bits(module).is_none());
        assert!(crate::intrinsics::registry::publish_python_native_namespace(py, provider, module));
        crate::molt_module_cache_set(name, module);
        assert!(!crate::exception_pending(py));
        Self {
            name,
            previous,
            module,
        }
    }

    pub(crate) fn bits(&self) -> u64 {
        self.module
    }
}

impl Drop for NativeProviderTestNamespace {
    fn drop(&mut self) {
        crate::with_gil_entry_nopanic!(py, {
            crate::clear_exception(py);
            crate::molt_module_cache_del(self.name);
            crate::clear_exception(py);
            if !crate::obj_from_bits(self.previous).is_none() {
                crate::molt_module_cache_set(self.name, self.previous);
            }
            for bits in [self.name, self.previous, self.module] {
                crate::dec_ref_bits(py, bits);
            }
        });
    }
}

/// Terminal lifecycle cases must own a process: Failed is deliberately not
/// resettable. The parent checks a real normal exit and the original diagnostic,
/// so the old destructor-abort behavior cannot satisfy these controls.
#[cfg(all(not(target_arch = "wasm32"), panic = "unwind"))]
#[test]
fn runtime_test_transactions_preserve_terminal_failures() {
    const MODE: &str = "MOLT_TEST_TRANSACTION_TERMINAL";
    const TEST: &str = "test_support::runtime_test_transactions_preserve_terminal_failures";
    const MODES: [&str; 9] = [
        "prior",
        "cleanup",
        "both",
        "cold-both",
        "body-only",
        "ordinary",
        "ordinary-return",
        "reentry",
        "healthy",
    ];
    if let Ok(mode) = std::env::var(MODE) {
        use crate::state::runtime_state::{
            molt_runtime_init, molt_runtime_shutdown, runtime_is_ready,
        };
        use molt_cpython_abi::api::{errors, pending_calls};
        assert_eq!(molt_runtime_init(), 1);
        let entered = Cell::new(false);
        let marker = Box::new(0x51a7_u64);
        let marker_address = (&*marker as *const u64).addr();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| match mode.as_str() {
            "prior" => {
                crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                RuntimeTestTransaction::with_trusted_fresh_runtime(|| entered.set(true));
            }
            "cleanup" | "both" | "body-only" => {
                RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
                    entered.set(true);
                    if mode != "body-only" {
                        crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                    }
                    if mode != "cleanup" {
                        std::panic::resume_unwind(marker);
                    }
                });
            }
            "cold-both" => {
                RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
                    assert_eq!(molt_runtime_init(), 1);
                    crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                    std::panic::resume_unwind(marker);
                });
            }
            "ordinary" | "ordinary-return" => {
                unsafe {
                    errors::PyErr_SetString(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                        c"owner detached before terminal failure".as_ptr(),
                    );
                }
                let _transaction = RuntimeTestTransaction::with_gc_isolation();
                entered.set(true);
                crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                assert_eq!(molt_runtime_shutdown(), 0);
                if mode == "ordinary" {
                    std::panic::resume_unwind(marker);
                }
            }
            "reentry" => {
                crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                assert_eq!(molt_runtime_shutdown(), 0);
                let _transaction = RuntimeTestTransaction::new();
                entered.set(true);
            }
            "healthy" => {
                unsafe {
                    errors::PyErr_SetString(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                        c"borrowed pre-transaction error".as_ptr(),
                    );
                }
                let original = errors::take_current_error().expect("actual C error owner");
                let identity = (original.exc_type, original.value, original.traceback);
                errors::restore_current_error_exact(original);
                {
                    let _transaction = RuntimeTestTransaction::with_gc_isolation();
                    assert!(errors::take_current_error().is_none());
                }
                let normal = errors::take_current_error().expect("normal restored C error");
                assert_eq!((normal.exc_type, normal.value, normal.traceback), identity);
                errors::restore_current_error_exact(normal);
                let failure = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let _transaction = RuntimeTestTransaction::with_gc_isolation();
                    assert!(errors::take_current_error().is_none());
                    std::panic::resume_unwind(marker);
                }))
                .expect_err("ordinary body panic must survive restoration");
                let restored = errors::take_current_error().expect("restored original C error");
                assert_eq!(
                    (restored.exc_type, restored.value, restored.traceback),
                    identity
                );
                drop(restored);
                assert!(runtime_is_ready());
                // Restoration must reopen exactly the prior producer admission.
                unsafe extern "C" fn no_op(_: *mut std::ffi::c_void) -> std::os::raw::c_int {
                    0
                }
                assert_eq!(
                    unsafe { pending_calls::Py_AddPendingCall(Some(no_op), std::ptr::null_mut()) },
                    0
                );
                crate::with_gil_entry_nopanic!(_py, {
                    assert_eq!(pending_calls::Py_MakePendingCalls(), 0);
                });
                std::panic::resume_unwind(failure);
            }
            _ => panic!("unknown transaction mode"),
        }));
        let failure = outcome.expect_err("failed transaction must never return success");
        if matches!(
            mode.as_str(),
            "both" | "cold-both" | "body-only" | "ordinary" | "healthy"
        ) {
            let original = failure
                .downcast::<u64>()
                .expect("original body panic payload");
            assert_eq!((&*original as *const u64).addr(), marker_address);
            assert_eq!(*original, 0x51a7);
        } else {
            let text = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .unwrap_or("");
            let expected = match mode.as_str() {
                "prior" => "could not retire the prior runtime",
                "cleanup" => "could not retire its runtime",
                "ordinary-return" => "lost its live runtime before restoration",
                "reentry" => "requires successful production bootstrap",
                _ => unreachable!(),
            };
            assert!(text.contains(expected), "{mode}: {text}");
        }
        if matches!(mode.as_str(), "prior" | "reentry") {
            assert!(!entered.get(), "body entered after failed admission");
        }
        if mode == "body-only" {
            assert_eq!(
                molt_runtime_init(),
                1,
                "ordinary panic must permit the next generation"
            );
        } else if mode != "healthy" {
            assert!(!runtime_is_ready());
            assert_eq!(molt_runtime_init(), 0, "terminal runtime must never revive");
            let rejected = std::panic::catch_unwind(|| {
                RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
                    panic!("terminal lifecycle admitted a body")
                });
            })
            .expect_err("a terminal lifecycle must not reopen test custody");
            let message = rejected
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| rejected.downcast_ref::<&str>().copied())
                .unwrap_or("");
            assert_eq!(
                message,
                "runtime lifecycle transaction requires a restartable runtime"
            );
            unsafe extern "C" fn forbidden(_: *mut std::ffi::c_void) -> std::os::raw::c_int {
                panic!("closed queue callback")
            }
            assert_eq!(
                unsafe { pending_calls::Py_AddPendingCall(Some(forbidden), std::ptr::null_mut()) },
                -1
            );
        }
        println!("transaction outcome and custody verified: {mode}");
        return;
    }
    for mode in MODES {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(MODE, mode);
        let output =
            captured_runtime_children::capture(&mut command, "runtime-test-transaction", mode);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{mode}: {stdout}\n{stderr}");
        assert!(
            stdout.contains(&format!("transaction outcome and custody verified: {mode}")),
            "{stdout}"
        );
        assert!(
            !stderr.contains("panic in a destructor during cleanup"),
            "{stderr}"
        );
        if !matches!(mode, "body-only" | "healthy") {
            assert!(stderr.contains("molt runtime lifecycle failed: injected shutdown drain C extension cleanup panic"), "{mode}: {stderr}");
        } else {
            assert!(
                !stderr.contains("molt runtime lifecycle failed:"),
                "{mode}: {stderr}"
            );
        }
        assert_eq!(
            stderr.contains("runtime test cleanup also failed:"),
            matches!(mode, "both" | "cold-both"),
            "{mode}: {stderr}"
        );
    }
}
