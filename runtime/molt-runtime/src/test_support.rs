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
    _process_state: MutexGuard<'static, ()>,
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
            _process_state: process_state,
        }
    }
}

impl Drop for RuntimeTestTransaction {
    fn drop(&mut self) {
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
