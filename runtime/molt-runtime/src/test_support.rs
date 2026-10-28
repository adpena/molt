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

// The library's libtest image is a WASI command. The selected SDK owns its
// startup object; Rust's bundled CRT is disabled by the common C-ABI plan.
// This module is cfg(test), so rlib/staticlib/cdylib consumers acquire no entry
// point. The generated declaration uses wasm-ld's exact-filename selection
// through the already authenticated native search path.
#[cfg(target_os = "wasi")]
include!(concat!(env!("OUT_DIR"), "/molt_wasi_test_startup.rs"));

#[allow(dead_code)]
pub(crate) mod cargo_test_artifacts {
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

/// Reproduce array.py's real producer boundary: a declared class publishes
/// native wrapper descriptors and an ordinary instance owns the storage handle.
/// The caller retains its handle; the returned instance is a new owned value.
pub(crate) fn declared_array_buffer_instance(py: &crate::PyToken<'_>, handle: u64) -> u64 {
    use crate::{MoltObject, dec_ref_bits, exception_pending, obj_from_bits};
    let name = crate::attr_name_bits_from_bytes(py, b"BufferContractArray").unwrap();
    let namespace = crate::alloc_dict_with_pairs(py, &[]);
    assert!(!namespace.is_null());
    let namespace = MoltObject::from_ptr(namespace).bits();
    let builtins = crate::builtin_classes(py);
    let class = crate::builtins::types::molt_type_new(
        builtins.type_obj,
        name,
        builtins.object,
        namespace,
        MoltObject::none().bits(),
    );
    assert!(!exception_pending(py));
    assert!(!obj_from_bits(class).is_none());
    dec_ref_bits(py, namespace);
    dec_ref_bits(py, name);
    crate::builtins::array_mod::molt_array_declare_buffer(class);
    assert!(!exception_pending(py));
    let instance =
        unsafe { crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
    assert!(!obj_from_bits(instance).is_none());
    let name = crate::attr_name_bits_from_bytes(py, b"_handle").unwrap();
    let result = crate::molt_set_attr_name(instance, name, handle);
    assert!(!exception_pending(py));
    dec_ref_bits(py, result);
    dec_ref_bits(py, name);
    dec_ref_bits(py, class);
    // Raw storage is still admitted by the internal physical C transport, but
    // public eligibility comes from the declared facade's semantic slots.
    assert_eq!(
        crate::object::buffer_exports::supports_buffer(py, handle),
        Ok(false)
    );
    assert_eq!(
        crate::object::buffer_exports::supports_buffer(py, instance),
        Ok(true)
    );
    instance
}

// Restore independent already-owned channels without manufacturing a new raise
// site. The matching/cleanup oracles intentionally retain both saved owners.
pub(crate) fn restore_runtime_raised(py: &crate::PyToken<'_>, bits: u64) {
    assert!(!crate::exception_pending(py));
    use crate::builtins::exceptions::{RaisedSnapshot, resolve_raised};
    crate::inc_ref_bits(py, bits);
    let mut saved = Some(match crate::current_task_key() {
        Some(task) => RaisedSnapshot::Task(task, bits),
        None => RaisedSnapshot::Thread(bits),
    });
    resolve_raised(py, &mut saved);
    assert!(saved.is_none());
}

/// Restore independently saved owners through the test transaction authority.
/// Both live channels must be empty; the runtime argument remains borrowed.
pub(crate) fn restore_pending_channels(
    py: &crate::PyToken<'_>,
    c_error: molt_cpython_abi::api::errors::OwnedCError,
    runtime_bits: u64,
) {
    assert!(
        !molt_cpython_abi::api::errors::raised_error_pending(),
        "both channels must be detached before setup"
    );
    restore_runtime_raised(py, runtime_bits);
    let mut saved = PendingExceptionSnapshot::detach();
    assert!(
        saved.c_error.is_none(),
        "C channel must be detached before setup"
    );
    saved.c_error = Some(c_error);
    saved.restore();
}

/// Observe the allocation-free raised channel through the real C API. Callers
/// install the hooks before denying allocation and keep the denial active here.
pub(crate) fn assert_and_clear_emergency_memory_error(py: &crate::PyToken<'_>) {
    use molt_cpython_abi::{
        abi_types,
        api::{errors, refcount},
    };
    assert!(crate::exception_pending(py));
    let bits =
        crate::exception_last_bits_noinc(py).expect("MemoryError is an owned reserved instance");
    unsafe {
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_MemoryError).cast()),
            1
        );
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_TypeError).cast()),
            0
        );
        assert_eq!(crate::exception_last_bits_noinc(py), Some(bits));
        let selected = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(
            !selected.as_ptr().is_null(),
            "zero-budget take transfers a real instance"
        );
        assert_eq!(
            (*selected.as_ptr()).ob_type,
            &raw mut abi_types::PyExc_MemoryError
        );
        assert!(!crate::exception_pending(py));
        let identity = selected.as_ptr();
        errors::PyErr_SetRaisedException(selected.into_ptr());
        let mut class = std::ptr::null_mut();
        let mut value = std::ptr::null_mut();
        let mut traceback = std::ptr::null_mut();
        errors::PyErr_Fetch(&raw mut class, &raw mut value, &raw mut traceback);
        let _class = refcount::OwnedPyObject::from_owned(class);
        let _value = refcount::OwnedPyObject::from_owned(value);
        let _traceback = refcount::OwnedPyObject::from_owned(traceback);
        assert_eq!(class, (&raw mut abi_types::PyExc_MemoryError).cast());
        assert_eq!(
            value, identity,
            "Fetch preserves the taken instance under allocation denial"
        );
        assert!(traceback.is_null());
        assert!(errors::PyErr_Occurred().is_null());
    }
    assert!(!crate::exception_pending(py));
}

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

/// Whether some test holds the process-global runtime test state. The mutex
/// admits one holder, so one flag describes it.
static PROCESS_TEST_CUSTODY_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Custody of the process-global runtime test state.
struct ProcessTestCustody {
    _state: MutexGuard<'static, ()>,
}

impl Drop for ProcessTestCustody {
    fn drop(&mut self) {
        // The flag clears before the lock releases (fields drop after this).
        PROCESS_TEST_CUSTODY_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    }
}

fn process_global_test_state() -> ProcessTestCustody {
    #[cfg(not(target_arch = "wasm32"))]
    assert!(
        !crate::concurrency::gil_held(),
        "runtime test transaction must acquire process-state custody before the GIL"
    );
    let state = PROCESS_GLOBAL_TEST_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    PROCESS_TEST_CUSTODY_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    ProcessTestCustody { _state: state }
}

/// Compiled code slots are one per-runtime table, sized once by its first
/// initialization. Every transaction sizes it to this capacity on entry and
/// empties every slot on exit, so no test reads a plan another test published
/// (HF-194). `publish_test_code_slot` publishes through the production
/// publisher (`molt_code_slot_set`, which derives the frame plan from the
/// current target) under an id no other test in this process uses.
const TEST_CODE_SLOT_CAPACITY: u64 = 4096;
static NEXT_TEST_CODE_SLOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Empty every published code slot of the live runtime.
fn clear_test_code_slots(py: &crate::PyToken<'_>) {
    if let Some(slots) = crate::runtime_state(py).code_slots.get() {
        for slot in slots {
            slot.clear(py);
        }
    }
}

/// Publish `code_bits` with `globals_bits` under a fresh slot id; return the id.
pub(crate) fn publish_test_code_slot(
    py: &crate::PyToken<'_>,
    code_bits: u64,
    globals_bits: u64,
) -> u64 {
    let slot = NEXT_TEST_CODE_SLOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert!(
        slot < TEST_CODE_SLOT_CAPACITY,
        "test code slots exhausted; raise TEST_CODE_SLOT_CAPACITY"
    );
    crate::molt_code_slots_init(TEST_CODE_SLOT_CAPACITY);
    crate::molt_code_slot_set(slot, code_bits, globals_bits);
    assert!(
        !crate::exception_pending(py),
        "test code slot {slot} publication failed"
    );
    slot
}

/// A runtime started while no test holds custody races the next test that
/// takes it: that test finds the runtime mid-transition (HF-194). Threads a
/// test spawns may start the runtime only while that test still holds custody.
pub(crate) fn require_custody_to_start_runtime() {
    if PROCESS_TEST_CUSTODY_ACTIVE.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let thread = std::thread::current();
    panic!(
        "thread '{}' started the runtime while no test held RuntimeTestTransaction \
         custody; the test that spawned it must keep custody until the thread ends\n{}",
        thread.name().unwrap_or("<unnamed>"),
        std::backtrace::Backtrace::force_capture()
    );
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
        let original = crate::exception_last_bits_noinc(py).expect("reserved owner");
        let snapshot = PendingExceptionSnapshot::detach();
        assert!(
            matches!(snapshot.runtime_error, Some(RaisedSnapshot::Thread(bits)) if bits == original)
        );
        assert!(!crate::exception_pending(py));
        crate::record_memory_error_without_allocation(py);
        snapshot.restore();
        assert!(crate::exception_pending(py));
        let restored = PendingExceptionSnapshot::detach();
        assert!(
            matches!(restored.runtime_error, Some(RaisedSnapshot::Thread(bits)) if bits == original)
        );
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
    /// Isolate teardown failures recorded before this transaction began.
    #[cfg(not(target_arch = "wasm32"))]
    isolate_teardown_failures: usize,
    _process_state: ProcessTestCustody,
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
        Self::with_runtime_lifecycle(RuntimeTestLifecycleMode::TrustedFresh, f, |value| value)
    }

    /// Like `with_trusted_fresh_runtime`, then run `epilogue` after the restart
    /// owner withdraws but before custody ends, for threads the body left
    /// waiting on the restart.
    pub(crate) fn with_trusted_fresh_runtime_then<R, T>(
        f: impl FnOnce() -> R,
        epilogue: impl FnOnce(R) -> T,
    ) -> T {
        Self::with_runtime_lifecycle(RuntimeTestLifecycleMode::TrustedFresh, f, epilogue)
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
        Self::with_runtime_lifecycle(RuntimeTestLifecycleMode::Cold, f, |value| value)
    }

    /// Exercise production admission with an exact grant set and no ambient tier.
    /// Environment mutation and the whole runtime lifecycle share process custody.
    pub(crate) fn with_explicit_capabilities(grants: &str, body: impl FnOnce(&crate::PyToken<'_>)) {
        Self::with_explicit_capabilities_using_init(
            grants,
            || crate::state::runtime_state::molt_runtime_init(),
            body,
        );
    }

    /// Supply real VFS backends at the production pre-Ready publication point.
    pub(crate) fn with_explicit_capabilities_and_vfs(
        grants: &str,
        vfs: Option<crate::vfs::VfsState>,
        body: impl FnOnce(&crate::PyToken<'_>),
    ) {
        Self::with_explicit_capabilities_using_init(
            grants,
            || crate::state::runtime_state::runtime_init_with_vfs_for_testing(vfs),
            body,
        );
    }

    fn with_explicit_capabilities_using_init(
        grants: &str,
        initialize: impl FnOnce() -> u64,
        body: impl FnOnce(&crate::PyToken<'_>),
    ) {
        Self::with_cold_runtime_lifecycle(|| {
            struct RestoreEnvironment([(&'static str, Option<std::ffi::OsString>); 2]);
            impl Drop for RestoreEnvironment {
                fn drop(&mut self) {
                    for (name, value) in &self.0 {
                        unsafe {
                            match value {
                                Some(value) => std::env::set_var(name, value),
                                None => std::env::remove_var(name),
                            }
                        }
                    }
                }
            }
            let _restore = RestoreEnvironment(
                ["MOLT_CAPABILITY_TIER", "MOLT_CAPABILITIES"]
                    .map(|name| (name, std::env::var_os(name))),
            );
            unsafe {
                std::env::set_var("MOLT_CAPABILITY_TIER", "none");
                std::env::set_var("MOLT_CAPABILITIES", grants);
            }
            assert_eq!(initialize(), 1);
            crate::with_gil_entry_nopanic!(py, { body(py) });
        });
    }

    fn with_runtime_lifecycle<R, T>(
        mode: RuntimeTestLifecycleMode,
        f: impl FnOnce() -> R,
        epilogue: impl FnOnce(R) -> T,
    ) -> T {
        let _process_state = process_global_test_state();
        assert!(
            crate::state::runtime_state::runtime_execution_is_admitted_for_current_thread(false),
            "runtime lifecycle transaction requires a restartable runtime ({})",
            crate::state::runtime_state::runtime_lifecycle_test_diagnostic()
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
        let value = match (outcome, cleanup) {
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
        };
        // Threads the body left waiting resume once the restart owner withdraws.
        // They finish here, still in custody. A runtime they start stays Ready
        // like any test's runtime; the next transaction retires it in custody.
        epilogue(value)
    }

    fn enter(isolate_gc: bool) -> Self {
        let process_state = process_global_test_state();
        #[cfg(not(target_arch = "wasm32"))]
        let isolate_teardown_failures =
            crate::concurrency::isolates::isolate_teardown_failure_count();
        let retained_thread_state_before =
            molt_cpython_abi::api::object::current_thread_has_retained_runtime_state();
        assert_eq!(
            crate::state::runtime_state::molt_runtime_init(),
            1,
            "runtime test transaction requires successful production bootstrap"
        );
        crate::molt_code_slots_init(TEST_CODE_SLOT_CAPACITY);
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
            #[cfg(not(target_arch = "wasm32"))]
            isolate_teardown_failures,
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
            self.interpreter_sys = None;
            self.gc = None;
            self.pending_calls.reset();
            assert!(
                std::thread::panicking(),
                "runtime test transaction lost its live runtime before restoration"
            );
            return;
        }
        crate::with_gil_entry_nopanic!(py, {
            clear_test_code_slots(py);
        });
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
        // A panicking isolate teardown only logs and retains the isolate, so
        // the test that started it would otherwise pass. Checked last, after
        // every restoration, so this failure cannot leak state into later tests.
        #[cfg(not(target_arch = "wasm32"))]
        if !std::thread::panicking() {
            assert_eq!(
                crate::concurrency::isolates::isolate_teardown_failure_count(),
                self.isolate_teardown_failures,
                "an isolate teardown panicked during this runtime test transaction"
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

/// Standalone cache/error tests supply the actual public resolver boundary.
/// The catalog is deliberately not consulted: an empty finder is a genuine
/// resolver miss, while ordinary cached values bypass this callback entirely.
/// Public util membership (including a preexisting None) is escrowed exactly.
pub(crate) struct EmptyImportResolver {
    previous: Option<u64>,
}

impl EmptyImportResolver {
    pub(crate) fn new(py: &crate::PyToken<'_>) -> Self {
        extern "C" fn find_spec(_name: u64, _package: u64) -> u64 {
            crate::MoltObject::none().bits()
        }
        use crate::{MoltObject, dec_ref_bits};
        let name = crate::attr_name_bits_from_bytes(py, b"importlib.util").unwrap();
        let module = crate::object::builders::alloc_module_obj(py, name);
        assert!(!module.is_null());
        let module = MoltObject::from_ptr(module).bits();
        let function = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "empty_public_import_finder",
                find_spec as *const (),
            ),
            2,
        );
        assert!(!function.is_null());
        let function = MoltObject::from_ptr(function).bits();
        let key = crate::attr_name_bits_from_bytes(py, b"find_spec").unwrap();
        crate::builtins::modules::molt_module_set_attr(module, key, function);
        assert!(!crate::exception_pending(py));
        let previous = crate::builtins::module_table::replace_module_owned_for_test(
            py,
            "importlib.util",
            Some(module),
        );
        // Replacement consumes the module owner; its dictionary owns function.
        for bits in [name, key, function] {
            dec_ref_bits(py, bits);
        }
        Self { previous }
    }
}

impl Drop for EmptyImportResolver {
    fn drop(&mut self) {
        crate::with_gil_entry_nopanic!(py, {
            let removed = crate::builtins::module_table::replace_module_owned_for_test(
                py,
                "importlib.util",
                self.previous.take(),
            );
            if let Some(bits) = removed {
                crate::dec_ref_bits(py, bits);
            }
        });
    }
}

/// Standalone runtime tests explicitly publish the same provider namespace as
/// a compiled initializer. This fixture owns/restores public membership. The
/// first builtins provider also establishes the interpreter role, whose owner
/// is escrowed by the surrounding RuntimeTestTransaction rather than selected
/// again by mutable public membership.
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
        if provider == "builtins" {
            crate::builtins::module_table::publish_interpreter_builtins_for_test(py, module);
        }
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
        let detached_native_error = Cell::new(0usize);
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
                crate::with_gil_entry_nopanic!(_py, {
                    unsafe {
                        errors::PyErr_SetString(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                            c"owner detached before terminal failure".as_ptr(),
                        );
                    }
                });
                let transaction = RuntimeTestTransaction::with_gc_isolation();
                let saved = transaction
                    .pending_exceptions
                    .as_ref()
                    .and_then(|snapshot| snapshot.c_error.as_ref())
                    .expect("transaction must own the original native C error");
                let address = saved.value.addr();
                assert!(crate::object::gc::native_gc_is_enrolled(address));
                assert!(unsafe { errors::PyErr_Occurred() }.is_null());
                detached_native_error.set(address);
                entered.set(true);
                // The detached native value is deliberately an external root.
                // The production retirement census must reject it before the
                // callback-free class tail; no later injected panic may replace
                // that primary failure.
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
                crate::with_gil_entry_nopanic!(_py, {
                    unsafe {
                        errors::PyErr_SetString(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                            c"borrowed pre-transaction error".as_ptr(),
                        );
                    }
                });
                let original = errors::take_current_error().expect("actual C error owner");
                let identity = (original.exc_type, original.value, original.traceback);
                errors::restore_current_error_exact(original);
                {
                    let _transaction = RuntimeTestTransaction::with_gc_isolation();
                    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
                }
                let normal = errors::take_current_error().expect("normal restored C error");
                assert_eq!((normal.exc_type, normal.value, normal.traceback), identity);
                errors::restore_current_error_exact(normal);
                let failure = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let _transaction = RuntimeTestTransaction::with_gc_isolation();
                    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
                    std::panic::resume_unwind(marker);
                }))
                .expect_err("ordinary body panic must survive restoration");
                let restored = errors::take_current_error().expect("restored original C error");
                assert_eq!(
                    (restored.exc_type, restored.value, restored.traceback),
                    identity
                );
                let released_native_error = restored.value.addr();
                assert!(crate::object::gc::native_gc_is_enrolled(
                    released_native_error
                ));
                crate::with_gil_entry_nopanic!(_py, {
                    assert_eq!(
                        unsafe { (*restored.value).ob_refcnt },
                        1,
                        "the restored native exception must have exactly its returned C owner"
                    );
                });
                drop(restored);
                assert!(!crate::object::gc::native_gc_is_enrolled(
                    released_native_error
                ));
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
                assert_eq!(
                    molt_runtime_shutdown(),
                    1,
                    "releasing the restored native owner must permit final retirement"
                );
                std::panic::resume_unwind(failure);
            }
            _ => panic!("unknown transaction mode"),
        }));
        let failure = outcome.expect_err("failed transaction must never return success");
        if matches!(mode.as_str(), "ordinary" | "ordinary-return") {
            // This is an opaque registry lookup, not a post-terminal object
            // dereference or runtime entry. Terminal cleanup must neither
            // republish nor release the detached native owner.
            assert!(crate::object::gc::native_gc_is_enrolled(
                detached_native_error.get()
            ));
        }
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
            assert!(
                message.starts_with(
                    "runtime lifecycle transaction requires a restartable runtime (phase Failed,"
                ),
                "{message}"
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
        if matches!(mode, "ordinary" | "ordinary-return") {
            assert!(
                stderr.contains("molt runtime lifecycle failed: native owners survived the last callback drain before class retirement"),
                "{mode}: {stderr}"
            );
            assert!(
                !stderr.contains("injected shutdown drain C extension cleanup panic"),
                "{mode}: {stderr}"
            );
        } else if !matches!(mode, "body-only" | "healthy") {
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
