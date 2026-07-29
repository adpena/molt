//! One execution-entry authority for native runtime callers.
//!
//! Lifecycle admission must happen before GIL acquisition. Otherwise a caller
//! can hold the outer GIL while waiting for `Finalizing`, deadlocking teardown
//! when it temporarily releases the GIL to join runtime workers. Every public
//! macro, Rust call boundary, extracted-core boundary, and persistent C GIL
//! boundary enters through the guards in this module.

use super::{GilGuard, PyToken, gil_held};
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

type PanicOutcome = Result<(), Box<dyn std::any::Any + Send + 'static>>;

#[derive(Clone, Copy)]
enum ExecutionDropPanicKind {
    Cleanup,
    Detach,
    Release,
    ShutdownCext,
    ShutdownDepth,
}

static EXECUTION_DROP_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);
static EXECUTION_DROP_CLEANUP_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);
static EXECUTION_DROP_DETACH_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);
static EXECUTION_DROP_RELEASE_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);
static SHUTDOWN_DROP_CEXT_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);
static SHUTDOWN_DROP_DEPTH_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionDropPanicDiagnostics {
    pub(crate) total: u64,
    pub(crate) cleanup: u64,
    pub(crate) detach: u64,
    pub(crate) release: u64,
    pub(crate) shutdown_cext: u64,
    pub(crate) shutdown_depth: u64,
}

pub(crate) fn execution_drop_panic_diagnostics() -> ExecutionDropPanicDiagnostics {
    ExecutionDropPanicDiagnostics {
        total: EXECUTION_DROP_PANIC_COUNT.load(Ordering::Relaxed) as u64,
        cleanup: EXECUTION_DROP_CLEANUP_PANIC_COUNT.load(Ordering::Relaxed) as u64,
        detach: EXECUTION_DROP_DETACH_PANIC_COUNT.load(Ordering::Relaxed) as u64,
        release: EXECUTION_DROP_RELEASE_PANIC_COUNT.load(Ordering::Relaxed) as u64,
        shutdown_cext: SHUTDOWN_DROP_CEXT_PANIC_COUNT.load(Ordering::Relaxed) as u64,
        shutdown_depth: SHUTDOWN_DROP_DEPTH_PANIC_COUNT.load(Ordering::Relaxed) as u64,
    }
}

fn record_execution_drop_panic(kind: ExecutionDropPanicKind) {
    EXECUTION_DROP_PANIC_COUNT.fetch_add(1, Ordering::Relaxed);
    let counter = match kind {
        ExecutionDropPanicKind::Cleanup => &EXECUTION_DROP_CLEANUP_PANIC_COUNT,
        ExecutionDropPanicKind::Detach => &EXECUTION_DROP_DETACH_PANIC_COUNT,
        ExecutionDropPanicKind::Release => &EXECUTION_DROP_RELEASE_PANIC_COUNT,
        ExecutionDropPanicKind::ShutdownCext => &SHUTDOWN_DROP_CEXT_PANIC_COUNT,
        ExecutionDropPanicKind::ShutdownDepth => &SHUTDOWN_DROP_DEPTH_PANIC_COUNT,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Complete every destruction obligation while preserving panic diagnostics.
///
/// A destructor entered during an existing unwind must not start a second
/// unwind: Rust aborts the process before the original panic reaches its
/// boundary. We therefore retain category/count telemetry and intentionally
/// leak only the caught panic payload on that exceptional path. Outside an
/// existing unwind, the first panic remains observable after all obligations
/// have run; later payloads are recorded and forgotten so dropping them cannot
/// create a second panic during `resume_unwind`.
fn finish_panic_obligations<const N: usize>(outcomes: [(ExecutionDropPanicKind, PanicOutcome); N]) {
    let already_unwinding = std::thread::panicking();
    let mut primary = None;
    for (kind, outcome) in outcomes {
        let Err(payload) = outcome else {
            continue;
        };
        record_execution_drop_panic(kind);
        if already_unwinding || primary.is_some() {
            std::mem::forget(payload);
        } else {
            primary = Some(payload);
        }
    }
    if let Some(payload) = primary {
        resume_unwind(payload);
    }
}

fn cleanup_detach_then_release(
    cleanup: impl FnOnce(),
    detach: impl FnOnce(),
    release: impl FnOnce(),
) {
    let cleanup_result = catch_unwind(AssertUnwindSafe(cleanup));
    let detach_result = catch_unwind(AssertUnwindSafe(detach));
    let release_result = catch_unwind(AssertUnwindSafe(release));
    finish_panic_obligations([
        (ExecutionDropPanicKind::Cleanup, cleanup_result),
        (ExecutionDropPanicKind::Detach, detach_result),
        (ExecutionDropPanicKind::Release, release_result),
    ]);
}

fn finish_then_release(finish: impl FnOnce(), release: impl FnOnce()) {
    let finish_result = catch_unwind(AssertUnwindSafe(finish));
    let release_result = catch_unwind(AssertUnwindSafe(release));
    finish_panic_obligations([
        (ExecutionDropPanicKind::Cleanup, finish_result),
        (ExecutionDropPanicKind::Release, release_result),
    ]);
}

fn finish_execution(
    custody: RuntimeExecutionCustody,
    py: &PyToken<'_>,
    active_lifecycle_lease: bool,
) {
    finish_then_release(
        || custody.finish(py),
        || {
            if active_lifecycle_lease {
                crate::state::runtime_state::release_runtime_execution_lease();
            }
        },
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn execution_is_nested() -> bool {
    gil_held()
}

#[cfg(target_arch = "wasm32")]
fn execution_is_nested() -> bool {
    WASM_RUNTIME_EXECUTION_DEPTH.with(|depth| depth.get() != 0)
}

/// Exact current-thread projection of an admitted runtime execution boundary.
///
/// This is intentionally stronger than runtime initialization.  In
/// particular, the wasm target is single-threaded today, but an initialized
/// isolate is not attached while the browser host is between app calls.
#[cfg(target_arch = "wasm32")]
pub(crate) fn current_thread_has_runtime_execution_custody() -> bool {
    (crate::state::runtime_state::current_thread_holds_runtime_execution_lease()
        || current_thread_holds_shutdown_drain_custody())
        && execution_is_nested()
}

fn current_thread_holds_shutdown_drain_custody() -> bool {
    SHUTDOWN_DRAIN_EXECUTION_DEPTH.with(|depth| depth.get() != 0)
}

#[inline(always)]
pub(crate) fn current_thread_has_c_extension_execution_context() -> bool {
    CEXT_EXECUTION_CONTEXT_DEPTH.with(|depth| depth.get() != 0)
}

fn enter_c_extension_execution_context() {
    assert!(gil_held(), "C extension execution context requires the GIL");
    CEXT_EXECUTION_CONTEXT_DEPTH.with(|depth| {
        depth.set(
            depth
                .get()
                .checked_add(1)
                .expect("C extension execution context depth overflow"),
        );
    });
}

fn leave_c_extension_execution_context() {
    CEXT_EXECUTION_CONTEXT_DEPTH.with(|depth| {
        let current = depth.get();
        if current != 0 {
            depth.set(current - 1);
        }
        assert_ne!(current, 0, "C extension execution context depth underflow");
    });
}

pub(crate) struct ShutdownDrainExecutionCustody {
    _private: (),
}

impl ShutdownDrainExecutionCustody {
    pub(crate) fn enter() -> Self {
        assert!(
            gil_held(),
            "shutdown drain execution custody requires the GIL"
        );
        assert!(
            !crate::state::runtime_state::current_thread_holds_runtime_execution_lease(),
            "shutdown drain custody must not overlap ordinary lifecycle admission"
        );
        SHUTDOWN_DRAIN_EXECUTION_DEPTH.with(|depth| {
            assert_eq!(depth.get(), 0, "duplicate shutdown drain execution custody");
            depth.set(1);
        });
        enter_c_extension_execution_context();
        Self { _private: () }
    }
}

impl Drop for ShutdownDrainExecutionCustody {
    fn drop(&mut self) {
        let cext_result = catch_unwind(AssertUnwindSafe(|| {
            leave_c_extension_execution_context();
            #[cfg(test)]
            RUNTIME_EXECUTION_SHUTDOWN_DROP_TEST_PANIC.with(|panic_next| {
                if panic_next.replace(false) {
                    panic!("injected shutdown drain C extension cleanup panic");
                }
            });
        }));
        let depth_result = catch_unwind(AssertUnwindSafe(|| {
            SHUTDOWN_DRAIN_EXECUTION_DEPTH.with(|depth| {
                let previous = depth.replace(0);
                assert_eq!(previous, 1, "shutdown drain execution custody corrupted");
            });
        }));
        finish_panic_obligations([
            (ExecutionDropPanicKind::ShutdownCext, cext_result),
            (ExecutionDropPanicKind::ShutdownDepth, depth_result),
        ]);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RuntimeExecutionCustody {
    clear_worker_state: bool,
    established_cext_context: bool,
    #[cfg(not(target_arch = "wasm32"))]
    destroy_thread_state: bool,
    #[cfg(not(target_arch = "wasm32"))]
    established_attachment: bool,
    #[cfg(target_arch = "wasm32")]
    established_execution_depth: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExecutionCleanupPolicy {
    Preserve,
    BoundaryCreated,
    WorkerEnd,
}

impl RuntimeExecutionCustody {
    fn inherited() -> Self {
        Self {
            clear_worker_state: false,
            established_cext_context: false,
            #[cfg(not(target_arch = "wasm32"))]
            destroy_thread_state: false,
            #[cfg(not(target_arch = "wasm32"))]
            established_attachment: false,
            #[cfg(target_arch = "wasm32")]
            established_execution_depth: false,
        }
    }

    fn enter(acquired_outer_gil: bool, cleanup_policy: ExecutionCleanupPolicy) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let attached_before =
                molt_cpython_abi::api::object::runtime_execution_thread_is_attached();
            if attached_before {
                enter_c_extension_execution_context();
                return Self {
                    clear_worker_state: false,
                    established_cext_context: true,
                    destroy_thread_state: false,
                    established_attachment: false,
                };
            }
            if !crate::state::runtime_state::runtime_is_initialized() {
                return Self {
                    clear_worker_state: false,
                    established_cext_context: false,
                    destroy_thread_state: false,
                    established_attachment: false,
                };
            }
            let created_by_boundary =
                molt_cpython_abi::api::object::attach_runtime_execution_thread();
            let established_attachment = !attached_before
                && molt_cpython_abi::api::object::runtime_execution_thread_is_attached();
            let owns_outer_boundary = acquired_outer_gil || established_attachment;
            enter_c_extension_execution_context();
            Self {
                clear_worker_state: cleanup_policy != ExecutionCleanupPolicy::Preserve
                    && owns_outer_boundary,
                destroy_thread_state: established_attachment
                    && match cleanup_policy {
                        ExecutionCleanupPolicy::Preserve => false,
                        ExecutionCleanupPolicy::BoundaryCreated => created_by_boundary,
                        ExecutionCleanupPolicy::WorkerEnd => true,
                    },
                established_attachment,
                established_cext_context: true,
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let was_outer = WASM_RUNTIME_EXECUTION_DEPTH.with(|depth| {
                let previous = depth.get();
                depth.set(
                    previous
                        .checked_add(1)
                        .expect("wasm runtime execution depth overflow"),
                );
                previous == 0
            });
            assert_eq!(
                acquired_outer_gil, was_outer,
                "logical wasm GIL custody diverged from execution nesting"
            );
            enter_c_extension_execution_context();
            Self {
                clear_worker_state: cleanup_policy != ExecutionCleanupPolicy::Preserve && was_outer,
                established_execution_depth: true,
                established_cext_context: true,
            }
        }
    }

    fn finish(self, py: &PyToken<'_>) {
        cleanup_detach_then_release(
            || {
                if self.clear_worker_state {
                    #[cfg(test)]
                    RUNTIME_EXECUTION_CLEANUP_TEST_PANIC.with(|panic_next| {
                        if panic_next.replace(false) {
                            panic!("injected execution cleanup panic");
                        }
                    });
                    crate::state::clear_worker_thread_state(py);
                }
            },
            || {
                #[cfg(not(target_arch = "wasm32"))]
                if self.established_attachment {
                    molt_cpython_abi::api::object::detach_runtime_execution_thread();
                    if self.destroy_thread_state {
                        molt_cpython_abi::api::object::clear_runtime_execution_thread_state();
                    }
                    #[cfg(test)]
                    RUNTIME_EXECUTION_DETACH_TEST_PANIC.with(|panic_next| {
                        if panic_next.replace(false) {
                            panic!("injected execution detach panic");
                        }
                    });
                }
                #[cfg(target_arch = "wasm32")]
                if self.established_execution_depth {
                    WASM_RUNTIME_EXECUTION_DEPTH.with(|depth| {
                        let previous = depth.get();
                        assert_ne!(previous, 0, "wasm runtime execution depth underflow");
                        depth.set(previous - 1);
                    });
                }
            },
            || {
                if self.established_cext_context {
                    leave_c_extension_execution_context();
                }
            },
        );
    }

    fn encoded_bits(self) -> u64 {
        let mut bits = u64::from(self.clear_worker_state);
        bits |= u64::from(self.established_cext_context) << 3;
        #[cfg(not(target_arch = "wasm32"))]
        {
            bits |= u64::from(self.established_attachment) << 1;
            bits |= u64::from(self.destroy_thread_state) << 2;
        }
        #[cfg(target_arch = "wasm32")]
        {
            bits |= u64::from(self.established_execution_depth) << 1;
        }
        bits
    }

    fn from_encoded_bits(bits: u64) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(
            bits & !0b1111,
            0,
            "invalid encoded execution custody {bits}"
        );
        #[cfg(target_arch = "wasm32")]
        assert_eq!(
            bits & !0b1011,
            0,
            "invalid encoded execution custody {bits}"
        );
        Self {
            clear_worker_state: bits & 1 != 0,
            established_cext_context: bits & 8 != 0,
            #[cfg(not(target_arch = "wasm32"))]
            destroy_thread_state: bits & 4 != 0,
            #[cfg(not(target_arch = "wasm32"))]
            established_attachment: bits & 2 != 0,
            #[cfg(target_arch = "wasm32")]
            established_execution_depth: bits & 2 != 0,
        }
    }
}

/// GIL plus the exact attachment and worker-TLS obligations established at a
/// public execution boundary.
pub(crate) struct RuntimeExecutionGuard {
    gil: Option<GilGuard>,
    custody: Option<RuntimeExecutionCustody>,
    active_lifecycle_lease: bool,
}

impl RuntimeExecutionGuard {
    /// Enter a runtime call, lazily initializing only before acquiring the GIL.
    pub(crate) fn enter() -> Self {
        Self::enter_with_policy(true, ExecutionCleanupPolicy::Preserve)
    }

    /// Enter a raw ABI call that is valid without bootstrapping RuntimeState.
    pub(crate) fn enter_without_runtime() -> Self {
        Self::enter_with_policy(false, ExecutionCleanupPolicy::Preserve)
    }

    pub(crate) fn enter_with_worker_cleanup() -> Self {
        Self::enter_with_policy(true, ExecutionCleanupPolicy::WorkerEnd)
    }

    pub(crate) fn enter_with_boundary_created_cleanup() -> Self {
        Self::enter_with_policy(true, ExecutionCleanupPolicy::BoundaryCreated)
    }

    fn enter_with_policy(initialize_runtime: bool, cleanup_policy: ExecutionCleanupPolicy) -> Self {
        if (crate::state::runtime_state::current_thread_holds_runtime_execution_lease()
            || current_thread_holds_shutdown_drain_custody())
            && let Some(gil) = GilGuard::new_if_held()
        {
            return Self {
                gil: Some(gil),
                custody: None,
                active_lifecycle_lease: false,
            };
        }
        if !crate::state::runtime_state::current_thread_holds_runtime_execution_lease() {
            crate::state::touch_tls_guard();
        }
        loop {
            let held_before = execution_is_nested();
            if !held_before
                && initialize_runtime
                && !crate::state::runtime_state::runtime_is_ready()
            {
                assert_ne!(
                    crate::state::runtime_state::molt_runtime_init(),
                    0,
                    "runtime execution entry attempted after permanent shutdown"
                );
            }
            if !held_before {
                crate::state::runtime_state::wait_for_runtime_execution_admission();
            }

            let active_lifecycle_lease =
                if crate::state::runtime_state::current_thread_holds_runtime_execution_lease() {
                    false
                } else {
                    let Some(acquired) =
                        crate::state::runtime_state::try_acquire_runtime_execution_lease(
                            initialize_runtime,
                        )
                    else {
                        assert!(
                            !held_before,
                            "nested runtime execution entry crossed closed lifecycle admission"
                        );
                        continue;
                    };
                    acquired
                };

            let gil = GilGuard::new();
            if crate::state::runtime_state::runtime_execution_is_admitted_for_current_thread(
                initialize_runtime,
            ) {
                let custody = RuntimeExecutionCustody::enter(!held_before, cleanup_policy);
                return Self {
                    gil: Some(gil),
                    custody: Some(custody),
                    active_lifecycle_lease,
                };
            }
            drop(gil);
            if active_lifecycle_lease {
                crate::state::runtime_state::release_runtime_execution_lease();
            }

            assert!(
                !held_before,
                "nested runtime execution entry crossed lifecycle finalization"
            );
            // Ready can be unpublished between the lock-free preflight and GIL
            // acquisition. Retry outside the GIL so lifecycle waiting cannot
            // participate in a lock cycle.
        }
    }

    /// Enter only if the runtime remains Ready after GIL acquisition.
    pub(crate) fn try_ready() -> Option<Self> {
        if !crate::state::runtime_state::runtime_is_ready() {
            return None;
        }
        crate::state::touch_tls_guard();
        let held_before = execution_is_nested();
        let active_lifecycle_lease =
            if crate::state::runtime_state::current_thread_holds_runtime_execution_lease() {
                false
            } else {
                crate::state::runtime_state::try_acquire_runtime_execution_lease(true)?
            };
        let gil = GilGuard::new();
        if !crate::state::runtime_state::runtime_is_ready() {
            if active_lifecycle_lease {
                crate::state::runtime_state::release_runtime_execution_lease();
            }
            return None;
        }
        let custody =
            RuntimeExecutionCustody::enter(!held_before, ExecutionCleanupPolicy::WorkerEnd);
        Some(Self {
            gil: Some(gil),
            custody: Some(custody),
            active_lifecycle_lease,
        })
    }

    pub(crate) fn token(&self) -> PyToken<'_> {
        self.gil.as_ref().expect("execution guard lost GIL").token()
    }

    pub(crate) fn into_encoded_lane(mut self) -> u64 {
        let custody = self
            .custody
            .take()
            .unwrap_or_else(RuntimeExecutionCustody::inherited);
        let gil = self.gil.take().expect("execution guard lost GIL");
        gil.into_encoded_lane()
            | ((custody.encoded_bits() | (u64::from(self.active_lifecycle_lease) << 4)) << 8)
    }

    /// Rebuild the exact guard returned by [`Self::into_encoded_lane`].
    ///
    /// # Safety
    ///
    /// `token` must be an unmatched token created on this thread and consumed
    /// exactly once.
    pub(crate) unsafe fn from_encoded_lane(token: u64) -> Self {
        let gil = unsafe { GilGuard::from_encoded_lane(token & 0xff) };
        let execution_bits = token >> 8;
        assert_eq!(
            execution_bits & !0b1_1111,
            0,
            "invalid encoded execution lane {execution_bits}"
        );
        let custody_bits = execution_bits & 0b1111;
        let custody =
            (custody_bits != 0).then(|| RuntimeExecutionCustody::from_encoded_bits(custody_bits));
        Self {
            gil: Some(gil),
            custody,
            active_lifecycle_lease: execution_bits & 0b1_0000 != 0,
        }
    }
}

impl Drop for RuntimeExecutionGuard {
    fn drop(&mut self) {
        if let Some(custody) = self.custody.take() {
            let gil = self.gil.as_ref().expect("execution custody lost GIL");
            finish_execution(custody, &gil.token(), self.active_lifecycle_lease);
            self.active_lifecycle_lease = false;
        }
    }
}

/// Enter an application execution boundary from a split/linked wasm host.
///
/// The returned token owns lifecycle admission, logical GIL custody, and the
/// target-specific attachment state.  It must be consumed exactly once by
/// [`molt_runtime_execution_leave`] on the same host thread.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_execution_enter() -> u64 {
    RuntimeExecutionGuard::enter().into_encoded_lane()
}

/// Leave the application execution boundary returned by
/// [`molt_runtime_execution_enter`].
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_execution_leave(token: u64) {
    assert_ne!(token, 0, "missing runtime execution boundary token");
    // SAFETY: the browser/embedding boundary consumes the same-thread token
    // exactly once in a `finally` path.
    drop(unsafe { RuntimeExecutionGuard::from_encoded_lane(token) });
}

/// Enter the one non-attaching destruction lane used by CPython thread-state
/// TLS. Unlike [`RuntimeExecutionGuard`], this acquires only lifecycle and GIL
/// custody: creating or attaching thread state here would recurse into the TLS
/// record currently being destroyed.
pub(crate) fn enter_retained_thread_state_drop() -> u64 {
    loop {
        let Some(active_lifecycle_lease) =
            crate::state::runtime_state::try_acquire_runtime_execution_lease(true)
        else {
            if !crate::state::runtime_state::runtime_is_initialized() {
                return 0;
            }
            std::thread::yield_now();
            continue;
        };
        let gil = GilGuard::new();
        if crate::state::runtime_state::runtime_state_for_gil().is_some() {
            return gil.into_encoded_lane() | (u64::from(active_lifecycle_lease) << 8);
        }
        drop(gil);
        if active_lifecycle_lease {
            crate::state::runtime_state::release_runtime_execution_lease();
        }
        if !crate::state::runtime_state::runtime_is_initialized() {
            return 0;
        }
    }
}

/// Release the exact token returned by [`enter_retained_thread_state_drop`].
///
/// # Safety
///
/// `token` must be nonzero, current-thread owned, and consumed exactly once.
pub(crate) unsafe fn leave_retained_thread_state_drop(token: u64) {
    assert_ne!(token, 0, "missing retained thread-state drop custody");
    assert_eq!(token & !0x1ff, 0, "invalid retained-state drop token");
    let active_lifecycle_lease = token & 0x100 != 0;
    let gil = unsafe { GilGuard::from_encoded_lane(token & 0xff) };
    if active_lifecycle_lease {
        crate::state::runtime_state::release_runtime_execution_lease();
    }
    drop(gil);
}

#[derive(Clone, Copy)]
struct PersistentRuntimeExecution {
    acquired_outer_gil: bool,
    custody: RuntimeExecutionCustody,
    active_lifecycle_lease: bool,
}

thread_local! {
    static PERSISTENT_RUNTIME_EXECUTION: Cell<Option<PersistentRuntimeExecution>> = const { Cell::new(None) };
    static SHUTDOWN_DRAIN_EXECUTION_DEPTH: Cell<u8> = const { Cell::new(0) };
    static CEXT_EXECUTION_CONTEXT_DEPTH: Cell<u32> = const { Cell::new(0) };
    #[cfg(target_arch = "wasm32")]
    static WASM_RUNTIME_EXECUTION_DEPTH: Cell<usize> = const { Cell::new(0) };
    #[cfg(test)]
    static RUNTIME_EXECUTION_CLEANUP_TEST_PANIC: Cell<bool> = const { Cell::new(false) };
    #[cfg(test)]
    static RUNTIME_EXECUTION_DETACH_TEST_PANIC: Cell<bool> = const { Cell::new(false) };
    #[cfg(test)]
    static RUNTIME_EXECUTION_SHUTDOWN_DROP_TEST_PANIC: Cell<bool> = const { Cell::new(false) };
}

/// Establish the persistent public C GIL boundary through the same lifecycle
/// admission and attachment authority as scoped entries.
pub(crate) fn ensure_persistent_runtime_execution() {
    if PERSISTENT_RUNTIME_EXECUTION.with(|slot| slot.get().is_some()) {
        return;
    }
    if execution_is_nested() {
        assert!(
            crate::state::runtime_state::runtime_state_for_gil().is_some(),
            "persistent runtime execution crossed lifecycle finalization"
        );
        let active_lifecycle_lease =
            crate::state::runtime_state::try_acquire_runtime_execution_lease(true)
                .expect("persistent runtime execution crossed closed lifecycle admission");
        let custody = RuntimeExecutionCustody::enter(false, ExecutionCleanupPolicy::WorkerEnd);
        PERSISTENT_RUNTIME_EXECUTION.with(|slot| {
            slot.set(Some(PersistentRuntimeExecution {
                acquired_outer_gil: false,
                custody,
                active_lifecycle_lease,
            }));
        });
        return;
    }

    let mut guard = RuntimeExecutionGuard::enter_with_worker_cleanup();
    let custody = guard
        .custody
        .take()
        .expect("persistent entry lost attachment custody");
    let active_lifecycle_lease = std::mem::replace(&mut guard.active_lifecycle_lease, false);
    let gil = guard.gil.take().expect("persistent entry lost GIL");
    crate::concurrency::gil::hold_runtime_gil(gil);
    PERSISTENT_RUNTIME_EXECUTION.with(|slot| {
        slot.set(Some(PersistentRuntimeExecution {
            acquired_outer_gil: true,
            custody,
            active_lifecycle_lease,
        }));
    });
}

pub(crate) fn release_persistent_runtime_execution() -> bool {
    let Some(execution) = PERSISTENT_RUNTIME_EXECUTION.with(|slot| slot.replace(None)) else {
        return false;
    };
    let gil = GilGuard::new();
    finish_then_release(
        || {
            finish_execution(
                execution.custody,
                &gil.token(),
                execution.active_lifecycle_lease,
            );
            drop(gil);
        },
        || {
            if execution.acquired_outer_gil {
                crate::concurrency::gil::release_runtime_gil();
            }
        },
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn scoped_cleanup_panic_detaches_and_releases_outer_gil() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let lease_baseline = crate::state::runtime_state::active_runtime_execution_lease_count();
        let guard = RuntimeExecutionGuard::enter_with_worker_cleanup();
        assert!(current_thread_has_c_extension_execution_context());
        assert!(
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "scoped execution guard must attach the current thread"
        );
        RUNTIME_EXECUTION_CLEANUP_TEST_PANIC.with(|panic_next| panic_next.set(true));
        let outcome = crate::test_support::catch_expected_unwind(|| drop(guard));
        assert!(outcome.is_err());
        assert!(
            !current_thread_has_c_extension_execution_context(),
            "cleanup panic must revoke C extension execution capability"
        );
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline,
            "cleanup panic must release independent lifecycle admission custody"
        );
        assert!(
            !molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "cleanup panic must not leak the scoped attachment"
        );

        let (entered_tx, entered_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _guard = RuntimeExecutionGuard::enter();
            entered_tx.send(()).unwrap();
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("cleanup panic must release the acquired outer GIL");
        worker.join().unwrap();
    }

    #[test]
    fn scoped_detach_panic_revokes_c_extension_execution_capability() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let lease_baseline = crate::state::runtime_state::active_runtime_execution_lease_count();
        let guard = RuntimeExecutionGuard::enter_with_worker_cleanup();
        assert!(current_thread_has_c_extension_execution_context());
        assert!(
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "scoped execution guard must attach before detach-panic proof"
        );
        RUNTIME_EXECUTION_DETACH_TEST_PANIC.with(|panic_next| panic_next.set(true));
        let outcome = crate::test_support::catch_expected_unwind(|| drop(guard));
        assert!(outcome.is_err());
        assert!(
            !current_thread_has_c_extension_execution_context(),
            "detach panic must revoke C extension execution capability"
        );
        assert!(
            !molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "detach panic proof must not leak the scoped attachment"
        );
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline,
            "detach panic must release independent lifecycle admission custody"
        );
    }

    #[test]
    fn scoped_cleanup_panic_during_outer_unwind_preserves_original_panic() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let diagnostics_before = execution_drop_panic_diagnostics();
        let lease_baseline = crate::state::runtime_state::active_runtime_execution_lease_count();
        let outcome = crate::test_support::catch_expected_unwind(|| {
            let _guard = RuntimeExecutionGuard::enter_with_worker_cleanup();
            RUNTIME_EXECUTION_CLEANUP_TEST_PANIC.with(|panic_next| panic_next.set(true));
            panic!("original scoped execution unwind");
        });
        let payload = outcome.expect_err("the original unwind must remain observable");
        assert_eq!(
            payload.downcast_ref::<&'static str>(),
            Some(&"original scoped execution unwind")
        );
        let diagnostics_after = execution_drop_panic_diagnostics();
        assert_eq!(diagnostics_after.total, diagnostics_before.total + 1);
        assert_eq!(diagnostics_after.cleanup, diagnostics_before.cleanup + 1);
        assert_eq!(diagnostics_after.detach, diagnostics_before.detach);
        assert_eq!(diagnostics_after.release, diagnostics_before.release);
        assert_eq!(
            diagnostics_after.shutdown_cext,
            diagnostics_before.shutdown_cext
        );
        assert_eq!(
            diagnostics_after.shutdown_depth,
            diagnostics_before.shutdown_depth
        );
        assert!(!current_thread_has_c_extension_execution_context());
        assert!(
            !molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "outer unwind cleanup must still detach its runtime attachment"
        );
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline,
            "outer unwind cleanup must still release lifecycle admission"
        );
    }

    #[test]
    fn shutdown_drain_drop_panic_during_outer_unwind_clears_all_custody() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let diagnostics_before = execution_drop_panic_diagnostics();
        let gil = GilGuard::new();
        let outcome = crate::test_support::catch_expected_unwind(|| {
            let _custody = ShutdownDrainExecutionCustody::enter();
            RUNTIME_EXECUTION_SHUTDOWN_DROP_TEST_PANIC.with(|panic_next| panic_next.set(true));
            panic!("original shutdown drain unwind");
        });
        let payload = outcome.expect_err("the original shutdown unwind must remain observable");
        assert_eq!(
            payload.downcast_ref::<&'static str>(),
            Some(&"original shutdown drain unwind")
        );
        let diagnostics_after = execution_drop_panic_diagnostics();
        assert_eq!(diagnostics_after.total, diagnostics_before.total + 1);
        assert_eq!(diagnostics_after.cleanup, diagnostics_before.cleanup);
        assert_eq!(diagnostics_after.detach, diagnostics_before.detach);
        assert_eq!(diagnostics_after.release, diagnostics_before.release);
        assert_eq!(
            diagnostics_after.shutdown_cext,
            diagnostics_before.shutdown_cext + 1
        );
        assert_eq!(
            diagnostics_after.shutdown_depth,
            diagnostics_before.shutdown_depth
        );
        assert!(!current_thread_has_c_extension_execution_context());
        assert!(!current_thread_holds_shutdown_drain_custody());
        let profile = crate::object::ops::runtime_profile_payload(&gil.token(), false);
        let safety = profile
            .get("execution_safety")
            .expect("runtime profile must expose execution-safety diagnostics");
        assert_eq!(
            safety
                .get("drop_panic_count")
                .and_then(|value| value.as_u64()),
            Some(diagnostics_after.total)
        );
        assert_eq!(
            safety
                .get("shutdown_drop_cext_panic_count")
                .and_then(|value| value.as_u64()),
            Some(diagnostics_after.shutdown_cext)
        );
        drop(gil);
    }

    #[test]
    fn persistent_cleanup_panic_detaches_and_releases_outer_gil() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        ensure_persistent_runtime_execution();
        assert!(current_thread_has_c_extension_execution_context());
        assert!(
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "persistent execution guard must attach the current thread"
        );
        RUNTIME_EXECUTION_CLEANUP_TEST_PANIC.with(|panic_next| panic_next.set(true));
        let outcome = crate::test_support::catch_expected_unwind(|| {
            release_persistent_runtime_execution();
        });
        assert!(outcome.is_err());
        assert!(
            !current_thread_has_c_extension_execution_context(),
            "persistent cleanup panic must revoke C extension execution capability"
        );
        assert!(
            !molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "cleanup panic must not leak the persistent attachment"
        );

        let (entered_tx, entered_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _guard = RuntimeExecutionGuard::enter();
            entered_tx.send(()).unwrap();
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("cleanup panic must release the persistent outer GIL");
        worker.join().unwrap();
    }

    #[test]
    fn nested_scoped_entry_updates_global_attachment_count_once() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let outer_gil = GilGuard::new();
        let baseline = molt_cpython_abi::api::object::runtime_execution_attachment_count();
        let lease_baseline = crate::state::runtime_state::active_runtime_execution_lease_count();
        let outer_entry = RuntimeExecutionGuard::enter();
        assert_eq!(
            molt_cpython_abi::api::object::runtime_execution_attachment_count(),
            baseline + 1
        );
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline + 1,
            "outer entry must acquire lifecycle custody before attachment"
        );
        let attempts_after_outer =
            crate::state::runtime_state::runtime_execution_lease_acquire_attempts();
        let inner_entry = RuntimeExecutionGuard::enter();
        let ready_entry = RuntimeExecutionGuard::try_ready().expect("nested Ready entry");
        let raw_entry = RuntimeExecutionGuard::enter_without_runtime();
        assert_eq!(
            molt_cpython_abi::api::object::runtime_execution_attachment_count(),
            baseline + 1,
            "nested entry must reuse the outer attachment"
        );
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline + 1,
            "nested execution must inherit the outer lifecycle lease"
        );
        assert_eq!(
            crate::state::runtime_state::runtime_execution_lease_acquire_attempts(),
            attempts_after_outer,
            "nested normal, try-ready, and raw entries must avoid global lease atomics"
        );
        drop(raw_entry);
        drop(ready_entry);
        drop(inner_entry);
        assert_eq!(
            molt_cpython_abi::api::object::runtime_execution_attachment_count(),
            baseline + 1
        );
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline + 1
        );
        drop(outer_entry);
        assert_eq!(
            molt_cpython_abi::api::object::runtime_execution_attachment_count(),
            baseline,
            "outer entry must detach exactly once"
        );
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline,
            "outer exit must release the final lifecycle lease"
        );
        drop(outer_gil);
    }

    #[test]
    fn ordinary_scoped_entries_preserve_the_c_error_indicator() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let first = RuntimeExecutionGuard::enter();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                    .cast::<molt_cpython_abi::abi_types::PyObject>(),
                c"persist across scoped calls".as_ptr(),
            );
        }
        drop(first);
        assert!(
            !molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "ordinary scoped exit must detach execution custody"
        );

        let second = RuntimeExecutionGuard::enter();
        assert_ne!(
            unsafe {
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                        .cast::<molt_cpython_abi::abi_types::PyObject>(),
                )
            },
            0,
            "a pending C error must survive to the next scoped C-API call"
        );
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        drop(second);
    }

    #[test]
    fn cleanup_owned_entry_destroys_retained_c_thread_state() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let producer = RuntimeExecutionGuard::enter();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                    .cast::<molt_cpython_abi::abi_types::PyObject>(),
                c"owned cleanup".as_ptr(),
            );
        }
        drop(producer);

        drop(RuntimeExecutionGuard::enter_with_worker_cleanup());

        let observer = RuntimeExecutionGuard::enter();
        assert!(
            unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
            "cleanup-owned boundary must destroy its retained C error state"
        );
        drop(observer);
    }

    #[test]
    fn thread_state_finalizer_panic_cannot_poison_drain_phase() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        std::thread::spawn(|| {
            let baseline = molt_cpython_abi::api::object::runtime_retained_thread_state_count();
            let producer = RuntimeExecutionGuard::enter();
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                        .cast::<molt_cpython_abi::abi_types::PyObject>(),
                    c"panic-safe retained-state cleanup".as_ptr(),
                );
            }
            drop(producer);
            molt_cpython_abi::api::object::inject_thread_state_finalizer_panic_for_test();

            assert!(
                crate::test_support::catch_expected_unwind(|| {
                    drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
                })
                .is_err(),
                "explicit cleanup must propagate an unknown Rust finalizer panic"
            );
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                baseline + 1,
                "panic-safe drain must leave the intact record available for retry"
            );
            drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                baseline,
                "a caught finalizer panic must still restore drain phase and retire the record"
            );

            let observer = RuntimeExecutionGuard::enter();
            assert!(
                unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
                "panic-safe drain must leave a reusable empty thread-state boundary"
            );
            drop(observer);
            drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn retained_thread_state_count_balances_explicit_and_tls_destruction() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let baseline = molt_cpython_abi::api::object::runtime_retained_thread_state_count();

        std::thread::spawn(move || {
            let guard = RuntimeExecutionGuard::enter();
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                        .cast::<molt_cpython_abi::abi_types::PyObject>(),
                    c"explicit retained-state cleanup".as_ptr(),
                );
            }
            drop(guard);
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                baseline + 1,
                "detached runtime-created state must remain counted"
            );
            drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                baseline,
                "explicit worker cleanup must retire exactly one retained owner"
            );
        })
        .join()
        .unwrap();

        std::thread::spawn(move || {
            let guard = RuntimeExecutionGuard::enter();
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                        .cast::<molt_cpython_abi::abi_types::PyObject>(),
                    c"TLS retained-state cleanup".as_ptr(),
                );
            }
            drop(guard);
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                baseline + 1
            );
            // Native TLS destruction owns the remaining count decrement.
        })
        .join()
        .unwrap();
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            baseline,
            "ThreadStateRecord::drop must balance native TLS destruction"
        );
    }

    #[test]
    fn retained_drop_reacquires_lease_after_ordinary_execution_tls_exit() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let retained_baseline =
            molt_cpython_abi::api::object::runtime_retained_thread_state_count();
        let lease_baseline = crate::state::runtime_state::active_runtime_execution_lease_count();
        std::thread::spawn(move || {
            let guard = RuntimeExecutionGuard::enter();
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                        .cast::<molt_cpython_abi::abi_types::PyObject>(),
                    c"TLS-order lease reacquisition".as_ptr(),
                );
            }
            drop(guard);
            assert_eq!(
                crate::state::runtime_state::active_runtime_execution_lease_count(),
                lease_baseline,
                "ordinary execution must release its outer lease before TLS exit"
            );
            // The armed CPython sentinel now acquires/releases a fresh retained-
            // drop lease after ordinary execution custody has gone away.
        })
        .join()
        .unwrap();
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline,
            "retained TLS drop must balance its independently reacquired lease"
        );
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            retained_baseline,
            "retained TLS drop must retire its published record"
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_logical_gil_and_execution_nesting_are_distinct() {
        assert!(gil_held(), "single-threaded wasm owns the logical GIL");
        assert!(
            !execution_is_nested(),
            "logical always-held GIL must not fabricate an execution frame"
        );
        let outer = RuntimeExecutionGuard::enter();
        assert!(execution_is_nested());
        let inner = RuntimeExecutionGuard::enter();
        drop(inner);
        assert!(
            execution_is_nested(),
            "nested exit must preserve the outer wasm execution frame"
        );
        drop(outer);
        assert!(!execution_is_nested());
    }
}
