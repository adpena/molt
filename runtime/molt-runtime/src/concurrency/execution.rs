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
    crate::state::execution_tls().wasm_execution_depth.get() != 0
}

/// Exact current-thread projection of an admitted runtime execution boundary.
///
/// This is intentionally stronger than runtime initialization.  In
/// particular, the wasm target is single-threaded today, but an initialized
/// isolate is not attached while the browser host is between app calls.
#[cfg(target_arch = "wasm32")]
pub(crate) fn current_thread_has_runtime_execution_custody() -> bool {
    runtime_execution_custody_in(crate::state::execution_tls())
}

/// [`current_thread_has_runtime_execution_custody`] for a caller that has
/// already read this thread's execution facts. wasm32 never writes the GIL
/// depth, so its logical execution depth stands in for GIL custody.
#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn runtime_execution_custody_in(tls: &crate::state::ExecutionTls) -> bool {
    // Shutdown owns a distinct lifetime capability, not an ordinary admitted
    // execution frame. Its callbacks must remain attached without fabricating
    // a runtime lease or WASM execution depth during finalization.
    tls.shutdown_drain_depth.get() != 0
        || (tls.lease_depth.get() != 0 && tls.wasm_execution_depth.get() != 0)
}

/// Admission custody (a lease or shutdown-drain custody) and the GIL: the
/// conditions under which today's re-entrant entry borrowed its caller's
/// custody. A nested entry under them owns nothing.
#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn runtime_execution_custody_in(tls: &crate::state::ExecutionTls) -> bool {
    (tls.lease_depth.get() != 0 || tls.shutdown_drain_depth.get() != 0) && tls.gil_depth.get() != 0
}

pub(crate) fn current_thread_holds_shutdown_drain_custody() -> bool {
    crate::state::execution_tls().shutdown_drain_depth.get() != 0
}

#[inline(always)]
pub(crate) fn current_thread_has_c_extension_execution_context() -> bool {
    crate::state::execution_tls().cext_execution_depth.get() != 0
}

/// Borrow the current execution admission for one compiled invocation.
///
/// This grants no ownership and cannot attach a thread. The invoker retains
/// its outer execution guard until compiled code returns; balanced C-API GIL
/// suspension must restore that guard before returning to compiled code.
/// Generator suspension returns to the invoker and a resume obtains a fresh
/// snapshot. A GIL-only caller, detached caller or diagnostic run uses the
/// existing checked runtime transitions instead of touching header cells.
pub(crate) fn molt_refcount_admission_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    refcount_admission_in(_py.execution_tls())
}

fn refcount_admission_in(tls: &crate::state::ExecutionTls) -> u64 {
    use molt_codegen_abi::inline_refcount::CompiledRefcountAdmission;
    if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC
        || tls.gil_depth.get() == 0
        || tls.cext_execution_depth.get() == 0
        || (tls.lease_depth.get() == 0 && tls.shutdown_drain_depth.get() == 0)
        || crate::object::rc_transition_trace_enabled()
    {
        CompiledRefcountAdmission::Runtime as u64
    } else {
        CompiledRefcountAdmission::AttachedPlain as u64
    }
}

fn enter_c_extension_execution_context() {
    assert!(gil_held(), "C extension execution context requires the GIL");
    {
        let depth = &crate::state::execution_tls().cext_execution_depth;
        depth.set(
            depth
                .get()
                .checked_add(1)
                .expect("C extension execution context depth overflow"),
        );
    }
}

fn leave_c_extension_execution_context() {
    {
        let depth = &crate::state::execution_tls().cext_execution_depth;
        let current = depth.get();
        if current != 0 {
            depth.set(current - 1);
        }
        assert_ne!(current, 0, "C extension execution context depth underflow");
    }
}

pub(crate) struct ShutdownDrainExecutionCustody {
    #[cfg(not(target_arch = "wasm32"))]
    retained_origin: bool,
}

impl ShutdownDrainExecutionCustody {
    /// Inherit the caller's lifetime capability or own the complete teardown.
    /// C-extension context alone is not a lease: bootstrap/finalizing entry can
    /// establish it without admitting ordinary runtime execution.
    pub(crate) fn enter_if_needed() -> Option<Self> {
        assert!(
            gil_held(),
            "shutdown drain execution custody requires the GIL"
        );
        if crate::state::runtime_state::current_thread_holds_runtime_execution_lease()
            || current_thread_holds_shutdown_drain_custody()
        {
            None
        } else {
            Some(Self::enter())
        }
    }

    pub(crate) fn enter() -> Self {
        assert!(
            gil_held(),
            "shutdown drain execution custody requires the GIL"
        );
        assert!(
            !crate::state::runtime_state::current_thread_holds_runtime_execution_lease(),
            "shutdown drain custody must not overlap ordinary lifecycle admission"
        );
        {
            let tls = crate::state::execution_tls();
            let depth = &tls.shutdown_drain_depth;
            assert_eq!(depth.get(), 0, "duplicate shutdown drain execution custody");
            depth.set(1);
            crate::state::runtime_state::bind_execution_state(tls);
        }
        enter_c_extension_execution_context();
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            retained_origin: false,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ShutdownDrainExecutionCustody {
    pub(crate) fn for_retained_worker(
        origin: crate::state::runtime_state::RuntimeWorkerOrigin,
    ) -> Self {
        // Record the origin before the custody edge binds this thread's
        // runtime state: during Finalizing only the origin resolves it.
        assert!(
            crate::state::execution_tls()
                .retained_worker_origin
                .replace(Some(origin))
                .is_none(),
            "duplicate retained worker origin"
        );
        let mut custody = Self::enter();
        custody.retained_origin = true;
        custody
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
            #[cfg(not(target_arch = "wasm32"))]
            if self.retained_origin {
                assert!(
                    crate::state::execution_tls()
                        .retained_worker_origin
                        .take()
                        .is_some(),
                    "retained worker origin lost"
                );
            }
            let tls = crate::state::execution_tls();
            let previous = tls.shutdown_drain_depth.replace(0);
            crate::state::runtime_state::release_execution_state_binding(tls);
            assert_eq!(previous, 1, "shutdown drain execution custody corrupted");
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
    destroy_thread_state: bool,
    established_attachment: bool,
    resumed_attachment: bool,
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
            destroy_thread_state: false,
            established_attachment: false,
            resumed_attachment: false,
        }
    }

    fn enter(acquired_outer_gil: bool, cleanup_policy: ExecutionCleanupPolicy) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let was_outer = {
                let depth = &crate::state::execution_tls().wasm_execution_depth;
                let previous = depth.get();
                depth.set(
                    previous
                        .checked_add(1)
                        .expect("wasm runtime execution depth overflow"),
                );
                previous == 0
            };
            assert_eq!(
                acquired_outer_gil, was_outer,
                "logical wasm GIL custody diverged from execution nesting"
            );
        }
        use molt_cpython_abi::api::object::BorrowedRuntimeThreadAttachment;
        let attachment = crate::state::runtime_state::runtime_state_for_gil()
            .map(|state| {
                molt_cpython_abi::api::object::borrow_runtime_execution_thread(state.identity())
            })
            .unwrap_or(BorrowedRuntimeThreadAttachment::Absent);
        if attachment != BorrowedRuntimeThreadAttachment::Absent {
            enter_c_extension_execution_context();
            return Self {
                clear_worker_state: false,
                established_cext_context: true,
                destroy_thread_state: false,
                established_attachment: false,
                resumed_attachment: attachment == BorrowedRuntimeThreadAttachment::Resumed,
            };
        }
        if !crate::state::runtime_state::runtime_is_initialized() {
            // Raw WASM entries still own logical nesting, but must not create
            // a C record or bootstrap an absent runtime.
            if cfg!(target_arch = "wasm32") {
                enter_c_extension_execution_context();
            }
            return Self {
                established_cext_context: cfg!(target_arch = "wasm32"),
                ..Self::inherited()
            };
        }
        let created_by_boundary = molt_cpython_abi::api::object::attach_runtime_execution_thread(
            crate::state::runtime_state::runtime_state_for_gil()
                .expect("live runtime execution owner")
                .identity(),
        );
        let established_attachment =
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached();
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
            resumed_attachment: false,
            established_cext_context: true,
        }
    }

    fn finish(self, py: &PyToken<'_>) {
        cleanup_detach_then_release(
            || {
                #[cfg(test)]
                if self.clear_worker_state {
                    RUNTIME_EXECUTION_CLEANUP_TEST_PANIC.with(|panic_next| {
                        if panic_next.replace(false) {
                            panic!("injected execution cleanup panic");
                        }
                    });
                }
                // An owned C record and runtime TLS reach quiescence together
                // after detach; retained-state boundaries keep their policy.
                if self.clear_worker_state && !self.destroy_thread_state {
                    crate::state::clear_worker_thread_state(py);
                }
            },
            || {
                if self.resumed_attachment {
                    molt_cpython_abi::api::object::suspend_resumed_runtime_execution_thread();
                }
                if self.established_attachment {
                    molt_cpython_abi::api::object::detach_runtime_execution_thread();
                    if self.destroy_thread_state {
                        molt_cpython_abi::api::object::clear_runtime_execution_thread_state(|| {
                            self.clear_worker_state && crate::state::clear_worker_thread_state(py)
                        });
                    }
                    #[cfg(test)]
                    RUNTIME_EXECUTION_DETACH_TEST_PANIC.with(|panic_next| {
                        if panic_next.replace(false) {
                            panic!("injected execution detach panic");
                        }
                    });
                }
            },
            || {
                let leave_context = || {
                    if self.established_cext_context {
                        leave_c_extension_execution_context();
                    }
                };
                // C-state drain can panic. Logical WASM depth and C-extension
                // context must still retire as independent obligations.
                #[cfg(target_arch = "wasm32")]
                finish_then_release(
                    || {
                        if self.established_cext_context {
                            {
                                let depth = &crate::state::execution_tls().wasm_execution_depth;
                                let previous = depth.get();
                                assert_ne!(previous, 0, "wasm runtime execution depth underflow");
                                depth.set(previous - 1);
                            }
                        }
                    },
                    leave_context,
                );
                #[cfg(not(target_arch = "wasm32"))]
                leave_context();
            },
        );
    }

    fn encoded_bits(self) -> u64 {
        u64::from(self.clear_worker_state)
            | (u64::from(self.established_attachment) << 1)
            | (u64::from(self.destroy_thread_state) << 2)
            | (u64::from(self.established_cext_context) << 3)
            | (u64::from(self.resumed_attachment) << 4)
    }

    fn from_encoded_bits(bits: u64) -> Self {
        assert_eq!(
            bits & !0b1_1111,
            0,
            "invalid encoded execution custody {bits}"
        );
        Self {
            clear_worker_state: bits & 1 != 0,
            established_attachment: bits & 2 != 0,
            destroy_thread_state: bits & 4 != 0,
            established_cext_context: bits & 8 != 0,
            resumed_attachment: bits & 16 != 0,
        }
    }
}

/// One service callback owns process entry and its exact origin borrow. Field
/// destruction completes WorkerEnd before releasing origin admission. Native
/// service failure remains owned by its retained JoinHandle through teardown.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RuntimeServiceExecutionGuard {
    execution: RuntimeExecutionGuard,
    _origin: crate::state::runtime_state::RuntimeOriginBorrow,
    _admission: RuntimeServiceAdmission,
}

// Release joined-service process admission after WorkerEnd and origin borrow.
#[cfg(not(target_arch = "wasm32"))]
struct RuntimeServiceAdmission(bool);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for RuntimeServiceAdmission {
    fn drop(&mut self) {
        if self.0 {
            crate::state::runtime_state::release_runtime_execution_lease();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeServiceExecutionGuard {
    pub(crate) fn try_enter(
        origin: crate::state::runtime_state::RuntimeWorkerOrigin,
    ) -> Option<Self> {
        // Process admission precedes the origin count: finalization cannot
        // observe a transient borrower whose process admission was refused.
        if !crate::state::runtime_state::runtime_is_ready() {
            return None;
        }
        let admission = RuntimeServiceAdmission(
            crate::state::runtime_state::try_acquire_runtime_service_execution_lease()?,
        );
        let origin_borrow =
            crate::state::runtime_state::RuntimeOriginBorrow::try_acquire(origin.state())?;
        let execution = RuntimeExecutionGuard::try_ready()?;
        // Closing an isolate's origin need not close the primary Ready state.
        // Reject a callback that was waiting for GIL across origin closure.
        if !origin.state().thread_borrow_admission_is_open() {
            return None;
        }
        assert_eq!(
            crate::runtime_state(execution.token()).identity(),
            origin.identity()
        );
        Some(Self {
            execution,
            _origin: origin_borrow,
            _admission: admission,
        })
    }

    #[inline(always)]
    pub(crate) fn token(&self) -> &PyToken<'_> {
        self.execution.token()
    }
}

/// GIL plus the exact attachment and worker-TLS obligations established at a
/// public execution boundary.
pub(crate) struct RuntimeExecutionGuard {
    /// This thread's execution facts, read once at entry.
    tls: &'static crate::state::ExecutionTls,
    /// What this entry acquired. `None` for a nested entry: its thread already
    /// held admission custody and the GIL, so the entry borrows both, adds no
    /// GIL depth unit, and its drop does nothing.
    owned: Option<OwnedExecution>,
}

/// The GIL guard and boundary obligations an entry acquired itself.
struct OwnedExecution {
    gil: GilGuard,
    custody: Option<RuntimeExecutionCustody>,
    active_lifecycle_lease: bool,
}

/// The encoded lane of a nested entry that borrowed its caller's custody. It
/// lies outside the GIL lane byte and the custody and lease bits, so no owned
/// lane can equal it.
const BORROWED_EXECUTION_LANE: u64 = 1 << 14;

impl RuntimeExecutionGuard {
    /// Enter a runtime call, lazily initializing only before acquiring the GIL.
    #[inline(always)]
    pub(crate) fn enter() -> Self {
        Self::enter_with_policy(true, ExecutionCleanupPolicy::Preserve)
    }

    /// Enter a raw ABI call that is valid without bootstrapping RuntimeState.
    #[inline(always)]
    pub(crate) fn enter_without_runtime() -> Self {
        Self::enter_with_policy(false, ExecutionCleanupPolicy::Preserve)
    }

    #[inline(always)]
    pub(crate) fn enter_with_worker_cleanup() -> Self {
        Self::enter_with_policy(true, ExecutionCleanupPolicy::WorkerEnd)
    }

    /// A foreign callback owns a real scope even when it borrows an existing
    /// attachment. SaveThread may have made that owner's C record dormant.
    pub(crate) fn enter_foreign_callback() -> Self {
        Self::enter_cold(
            crate::state::execution_tls(),
            true,
            ExecutionCleanupPolicy::BoundaryCreated,
        )
    }

    /// Compiled code enters here on every runtime operation, already holding
    /// admission custody and the GIL: one thread-local read answers both, and
    /// the entry borrows them, so it updates no depth and its drop does
    /// nothing. Everything else is the cold path, kept out of line so its frame
    /// and saved registers stay off the hot one.
    #[inline(always)]
    fn enter_with_policy(initialize_runtime: bool, cleanup_policy: ExecutionCleanupPolicy) -> Self {
        let tls = crate::state::execution_tls();
        if runtime_execution_custody_in(tls) {
            return Self { tls, owned: None };
        }
        Self::enter_cold(tls, initialize_runtime, cleanup_policy)
    }

    #[inline(always)]
    fn owned(
        tls: &'static crate::state::ExecutionTls,
        gil: GilGuard,
        custody: Option<RuntimeExecutionCustody>,
        active_lifecycle_lease: bool,
    ) -> Self {
        Self {
            tls,
            owned: Some(OwnedExecution {
                gil,
                custody,
                active_lifecycle_lease,
            }),
        }
    }

    #[cold]
    #[inline(never)]
    fn enter_cold(
        tls: &'static crate::state::ExecutionTls,
        initialize_runtime: bool,
        cleanup_policy: ExecutionCleanupPolicy,
    ) -> Self {
        let cleanup_policy = if cleanup_policy == ExecutionCleanupPolicy::Preserve
            && molt_cpython_abi::api::object::runtime_thread_state_lifetime_is_retired()
        {
            // The actual C lifetime key is gone. A late foreign boundary owns
            // any record it creates through cleanup, not another TLS sentinel.
            ExecutionCleanupPolicy::BoundaryCreated
        } else {
            cleanup_policy
        };
        if tls.lease_depth.get() != 0 || tls.shutdown_drain_depth.get() != 0 {
            // A forced foreign boundary owns its scope even if it borrows
            // current lifetime admission. On WASM an admitted owner may also
            // be outside a logical execution frame.
            if let Some(gil) = GilGuard::new_if_held_in(tls) {
                let custody =
                    RuntimeExecutionCustody::enter(!execution_is_nested(), cleanup_policy);
                return Self::owned(tls, gil, Some(custody), false);
            }
            // Admission/retained shutdown custody survives GIL suspension.
            // Reentry must not wait for Ready: the finalization owner may be
            // joining this very worker. Own only newly acquired GIL/attachment
            // obligations and preserve the caller's requested cleanup policy.
            let gil = GilGuard::new();
            let custody = RuntimeExecutionCustody::enter(true, cleanup_policy);
            return Self::owned(tls, gil, Some(custody), false);
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
                #[cfg(test)]
                crate::test_support::require_custody_to_start_runtime();
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
                return Self::owned(tls, gil, Some(custody), active_lifecycle_lease);
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
        Some(Self::owned(
            crate::state::execution_tls(),
            gil,
            Some(custody),
            active_lifecycle_lease,
        ))
    }

    #[inline(always)]
    pub(crate) fn token(&self) -> &PyToken<'_> {
        // SAFETY: the entry holds or borrows this thread's GIL custody for as
        // long as it lives, and `tls` is this thread's record.
        unsafe { PyToken::with_custody(self.tls) }
    }

    pub(crate) fn into_encoded_lane(mut self) -> u64 {
        let Some(owned) = self.owned.take() else {
            return BORROWED_EXECUTION_LANE;
        };
        let OwnedExecution {
            gil,
            custody,
            active_lifecycle_lease,
        } = owned;
        let custody = custody.unwrap_or_else(RuntimeExecutionCustody::inherited);
        gil.into_encoded_lane()
            | ((custody.encoded_bits() | (u64::from(active_lifecycle_lease) << 5)) << 8)
    }

    /// Rebuild the exact guard returned by [`Self::into_encoded_lane`].
    ///
    /// # Safety
    ///
    /// `token` must be an unmatched token created on this thread and consumed
    /// exactly once.
    pub(crate) unsafe fn from_encoded_lane(token: u64) -> Self {
        let tls = crate::state::execution_tls();
        if token == BORROWED_EXECUTION_LANE {
            return Self { tls, owned: None };
        }
        let gil = unsafe { GilGuard::from_encoded_lane(token & 0xff) };
        let execution_bits = token >> 8;
        assert_eq!(
            execution_bits & !0b11_1111,
            0,
            "invalid encoded execution lane {execution_bits}"
        );
        let custody_bits = execution_bits & 0b1_1111;
        let custody =
            (custody_bits != 0).then(|| RuntimeExecutionCustody::from_encoded_bits(custody_bits));
        Self::owned(tls, gil, custody, execution_bits & 0b10_0000 != 0)
    }
}

impl Drop for RuntimeExecutionGuard {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(owned) = self.owned.as_mut()
            && owned.custody.is_some()
        {
            Self::finish_cold(owned);
        }
    }
}

impl RuntimeExecutionGuard {
    #[cold]
    #[inline(never)]
    fn finish_cold(owned: &mut OwnedExecution) {
        let custody = owned
            .custody
            .take()
            .expect("execution custody finished twice");
        finish_execution(custody, owned.gil.token(), owned.active_lifecycle_lease);
        owned.active_lifecycle_lease = false;
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

/// Own normal execution custody for destruction of one exact retained C
/// record. The C drain makes that existing record current; creating another
/// record here would recurse into the ownership domain being destroyed.
pub(crate) fn enter_retained_thread_state_drop(runtime_owner: u64) -> u64 {
    assert_ne!(
        runtime_owner, 0,
        "thread-state destruction requires its exact origin"
    );
    loop {
        let tls = crate::state::execution_tls();
        let active_lifecycle_lease =
            if tls.lease_depth.get() != 0 || tls.shutdown_drain_depth.get() != 0 {
                // A suspended ordinary/drain owner retains this allocation.
                // Independently reacquire its GIL below, never wait for Ready.
                false
            } else {
                let Some(acquired) =
                    crate::state::runtime_state::try_acquire_runtime_execution_lease(true)
                else {
                    if !crate::state::runtime_state::runtime_is_initialized() {
                        return 0;
                    }
                    std::thread::yield_now();
                    continue;
                };
                acquired
            };
        let gil = GilGuard::new();
        let (resolved, matches_origin) = crate::state::runtime_state::runtime_state_for_gil()
            .map_or((false, false), |state| {
                (true, state.identity() as u64 == runtime_owner)
            });
        if matches_origin {
            enter_c_extension_execution_context();
            return gil.into_encoded_lane() | (u64::from(active_lifecycle_lease) << 8);
        }
        drop(gil);
        if active_lifecycle_lease {
            crate::state::runtime_state::release_runtime_execution_lease();
        }
        if resolved {
            // A different Ready runtime cannot license this record's payload.
            return 0;
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
    assert!(
        gil_held(),
        "thread-state finalizer returned without restoring the GIL"
    );
    leave_c_extension_execution_context();
    if active_lifecycle_lease {
        crate::state::runtime_state::release_runtime_execution_lease();
    }
    drop(gil);
}

/// Exact ownership of one compiled invocation. This value moves freely: only
/// typed scalar identities, never its stack address, are published in the record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompiledScopeId(u64);

#[derive(Clone, Copy)]
pub(crate) struct CompiledScopeFrame {
    id: CompiledScopeId,
    previous: u64,
    runtime: *const crate::state::runtime_state::RuntimeState,
    gil_depth: usize,
    lease_depth: usize,
    shutdown_depth: u8,
    public_depth: u32,
    public_owns_persistent: bool,
    cext_depth: u32,
    persistent_owner: u64,
    suspension: u64,
    depends_on_persistent: bool,
}

#[cold]
pub(crate) fn invalid_execution_restore(message: &str) -> ! {
    eprintln!("molt fatal: invalid execution custody restore: {message}");
    std::process::abort()
}

pub(crate) fn next_execution_identity(tls: &crate::state::ExecutionTls) -> u64 {
    let next = tls
        .execution_identity
        .get()
        .checked_add(1)
        .unwrap_or_else(|| invalid_execution_restore("execution identity exhausted"));
    tls.execution_identity.set(next);
    next
}

fn current_suspension(tls: &crate::state::ExecutionTls) -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tls.native_gil.suspension_identity()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tls;
        0
    }
}

impl CompiledScopeFrame {
    fn enter(py: &PyToken<'_>) -> Self {
        let tls = py.execution_tls();
        if !runtime_execution_custody_in(tls) || tls.bound_state.get().is_null() {
            invalid_execution_restore("compiled entry lacks admitted runtime/GIL custody");
        }
        let id = CompiledScopeId(next_execution_identity(tls));
        let persistent_owner = tls.persistent_execution_owner.get();
        let suspension = current_suspension(tls);
        let depends_on_persistent = persistent_owner != 0
            && tls.persistent_execution_owns_gil.get()
            && tls.gil_depth.get() == 1
            && suspension == 0;
        let frame = Self {
            id,
            previous: tls.compiled_scope_top.get(),
            runtime: tls.bound_state.get(),
            gil_depth: tls.gil_depth.get(),
            lease_depth: tls.lease_depth.get(),
            shutdown_depth: tls.shutdown_drain_depth.get(),
            public_depth: tls.public_gil_depth.get(),
            public_owns_persistent: tls.public_gil_owns_persistent.get(),
            persistent_owner,
            suspension,
            depends_on_persistent,
            cext_depth: tls.cext_execution_depth.get(),
        };
        let depth = tls
            .compiled_depth
            .get()
            .checked_add(1)
            .unwrap_or_else(|| invalid_execution_restore("compiled scope depth overflow"));
        if depends_on_persistent {
            tls.compiled_public_dependencies.set(
                tls.compiled_public_dependencies
                    .get()
                    .checked_add(1)
                    .unwrap_or_else(|| {
                        invalid_execution_restore("compiled owner dependency overflow")
                    }),
            );
        }
        tls.compiled_scope_top.set(id.0);
        tls.compiled_depth.set(depth);
        frame
    }

    fn validate(&self, tls: &crate::state::ExecutionTls) {
        if tls.compiled_scope_top.get() != self.id.0
            || tls.bound_state.get() != self.runtime
            || tls.gil_depth.get() != self.gil_depth
            || tls.lease_depth.get() != self.lease_depth
            || tls.shutdown_drain_depth.get() != self.shutdown_depth
            || tls.public_gil_depth.get() != self.public_depth
            || tls.public_gil_owns_persistent.get() != self.public_owns_persistent
            || tls.cext_execution_depth.get() != self.cext_depth
            || tls.persistent_execution_owner.get() != self.persistent_owner
            || current_suspension(tls) != self.suspension
            || !runtime_execution_custody_in(tls)
        {
            invalid_execution_restore("compiled scope identity/runtime/GIL/public custody changed");
        }
    }

    fn leave(self, tls: &crate::state::ExecutionTls) {
        self.validate(tls);
        tls.compiled_scope_top.set(self.previous);
        tls.compiled_depth.set(
            tls.compiled_depth
                .get()
                .checked_sub(1)
                .unwrap_or_else(|| invalid_execution_restore("compiled scope depth underflow")),
        );
        if self.depends_on_persistent {
            tls.compiled_public_dependencies.set(
                tls.compiled_public_dependencies
                    .get()
                    .checked_sub(1)
                    .unwrap_or_else(|| {
                        invalid_execution_restore("compiled owner dependency underflow")
                    }),
            );
        }
    }
}

pub(crate) struct CompiledScopeGuard<'scope, 'py> {
    py: &'scope PyToken<'py>,
    frame: CompiledScopeFrame,
}
impl<'scope, 'py> CompiledScopeGuard<'scope, 'py> {
    pub(crate) fn enter(py: &'scope PyToken<'py>) -> Self {
        Self {
            py,
            frame: CompiledScopeFrame::enter(py),
        }
    }
    pub(crate) fn validate(&self) {
        self.frame.validate(self.py.execution_tls());
    }
}
impl Drop for CompiledScopeGuard<'_, '_> {
    fn drop(&mut self) {
        self.frame.leave(self.py.execution_tls());
    }
}

/// Host boundary: a scope id owns one retained frame, closed exactly in LIFO order.
#[unsafe(no_mangle)]
pub extern "C" fn molt_compiled_execution_enter(context: u64) -> u64 {
    let py = unsafe { PyToken::from_attached_context(context) };
    let tls = py.execution_tls();
    if !std::ptr::eq(tls, crate::state::execution_tls()) {
        invalid_execution_restore("compiled host entry used another thread record");
    }
    // Reserve before publishing the scope so allocation failure cannot strand it.
    tls.compiled_host_scopes.borrow_mut().reserve(1);
    let frame = CompiledScopeFrame::enter(py);
    tls.compiled_host_scopes.borrow_mut().push(frame);
    frame.id.0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compiled_execution_leave(context: u64, identity: u64) {
    let py = unsafe { PyToken::from_attached_context(context) };
    let tls = py.execution_tls();
    if !std::ptr::eq(tls, crate::state::execution_tls()) {
        invalid_execution_restore("compiled host leave used another thread record");
    }
    let frame = tls
        .compiled_host_scopes
        .borrow()
        .last()
        .copied()
        .unwrap_or_else(|| invalid_execution_restore("compiled host scope is absent"));
    if frame.id.0 != identity {
        invalid_execution_restore("compiled host scope is not LIFO");
    }
    frame.validate(tls);
    let _ = tls.compiled_host_scopes.borrow_mut().pop();
    frame.leave(tls);
}

pub(crate) fn retire_idle_compiled_scope_storage() {
    let tls = crate::state::execution_tls();
    if tls.compiled_depth.get() == 0 {
        let mut scopes = tls.compiled_host_scopes.borrow_mut();
        if !scopes.is_empty() {
            invalid_execution_restore("idle compiled host scope remained owned");
        }
        *scopes = Vec::new();
    }
}

pub(crate) fn has_persistent_runtime_execution() -> bool {
    PERSISTENT_RUNTIME_EXECUTION.with(|slot| slot.get().is_some())
}

#[derive(Clone, Copy)]
struct PersistentRuntimeExecution {
    acquired_outer_gil: bool,
    custody: RuntimeExecutionCustody,
    active_lifecycle_lease: bool,
}

thread_local! {
    static PERSISTENT_RUNTIME_EXECUTION: Cell<Option<PersistentRuntimeExecution>> = const { Cell::new(None) };
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
    if has_persistent_runtime_execution() {
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
        let tls = crate::state::execution_tls();
        tls.persistent_execution_owner
            .set(next_execution_identity(tls));
        tls.persistent_execution_owns_gil.set(false);
        return;
    }

    let mut guard = RuntimeExecutionGuard::enter_with_worker_cleanup();
    let OwnedExecution {
        gil,
        custody,
        active_lifecycle_lease,
    } = guard
        .owned
        .take()
        .expect("persistent entry borrowed existing execution custody");
    let custody = custody.expect("persistent entry lost attachment custody");
    crate::concurrency::gil::hold_runtime_gil(gil);
    PERSISTENT_RUNTIME_EXECUTION.with(|slot| {
        slot.set(Some(PersistentRuntimeExecution {
            acquired_outer_gil: true,
            custody,
            active_lifecycle_lease,
        }));
    });
    let tls = crate::state::execution_tls();
    tls.persistent_execution_owner
        .set(next_execution_identity(tls));
    tls.persistent_execution_owns_gil.set(true);
}

#[cfg(all(test, panic = "unwind"))]
pub(crate) fn inject_shutdown_drain_drop_panic() {
    RUNTIME_EXECUTION_SHUTDOWN_DROP_TEST_PANIC.with(|pending| pending.set(true));
}

pub(crate) fn release_persistent_runtime_execution() -> bool {
    let tls = crate::state::execution_tls();
    if tls.compiled_public_dependencies.get() != 0
        || (tls.persistent_execution_owns_gil.get() && current_suspension(tls) != 0)
    {
        return false;
    }
    let Some(execution) = PERSISTENT_RUNTIME_EXECUTION.with(|slot| slot.replace(None)) else {
        return false;
    };
    tls.persistent_execution_owner.set(0);
    tls.persistent_execution_owns_gil.set(false);
    let gil = GilGuard::new();
    finish_then_release(
        || {
            finish_execution(
                execution.custody,
                gil.token(),
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

include!("../attached_exports_generated/concurrency/execution.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn compiled_refcount_admission_requires_full_live_execution_custody() {
        use molt_codegen_abi::inline_refcount::CompiledRefcountAdmission;
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let runtime = CompiledRefcountAdmission::Runtime as u64;
        assert_eq!(
            refcount_admission_in(crate::state::execution_tls()),
            runtime
        );
        {
            let _gil_only = GilGuard::new();
            assert!(gil_held());
            assert_eq!(
                refcount_admission_in(crate::state::execution_tls()),
                runtime,
                "a GIL token grants no attachment or lifecycle authority"
            );
        }
        {
            let _execution = RuntimeExecutionGuard::enter_with_worker_cleanup();
            assert!(current_thread_has_c_extension_execution_context());
            assert!(
                !crate::object::rc_transition_trace_enabled(),
                "this plain transition cell requires RC diagnostics disabled"
            );
            let expected = if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC {
                runtime
            } else {
                CompiledRefcountAdmission::AttachedPlain as u64
            };
            assert_eq!(
                refcount_admission_in(crate::state::execution_tls()),
                expected
            );
            {
                let _suspended = crate::GilReleaseGuard::suspend();
                assert_eq!(
                    refcount_admission_in(crate::state::execution_tls()),
                    runtime
                );
            }
            assert_eq!(
                refcount_admission_in(crate::state::execution_tls()),
                expected,
                "resumption borrows a fresh admission"
            );
            {
                let _nested = RuntimeExecutionGuard::enter();
                assert_eq!(
                    refcount_admission_in(crate::state::execution_tls()),
                    expected
                );
            }
            assert_eq!(
                refcount_admission_in(crate::state::execution_tls()),
                expected
            );
        }
        assert_eq!(
            refcount_admission_in(crate::state::execution_tls()),
            runtime,
            "admission expires with its execution boundary"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn nested_entry_borrows_custody_without_a_depth_unit() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        std::thread::spawn(|| {
            let tls = crate::state::execution_tls();
            let outer = RuntimeExecutionGuard::enter_with_worker_cleanup();
            assert!(
                outer.owned.is_some(),
                "the first entry acquires its custody"
            );
            let depth = tls.gil_depth.get();
            {
                let nested = RuntimeExecutionGuard::enter();
                assert!(nested.owned.is_none());
                assert_eq!(tls.gil_depth.get(), depth, "a borrow adds no depth unit");
                let lane = nested.into_encoded_lane();
                assert_eq!(lane, BORROWED_EXECUTION_LANE);
                let rebuilt = unsafe { RuntimeExecutionGuard::from_encoded_lane(lane) };
                assert!(rebuilt.owned.is_none());
            }
            assert_eq!(
                tls.gil_depth.get(),
                depth,
                "a borrow's drop releases nothing"
            );
            drop(outer);
            assert_eq!(tls.gil_depth.get(), 0);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn published_poll_word(record: u64) -> u64 {
        let address = record + molt_codegen_abi::THREAD_RECORD_PENDING_WORK_PTR_OFFSET as u64;
        // Read the physical ABI consumed by compiled native and LLVM polls.
        unsafe { std::ptr::with_exposed_provenance::<u64>(address as usize).read() }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn compiled_poll_word(py: &crate::PyToken<'_>) -> u64 {
        let address =
            py.attached_context() + molt_codegen_abi::ATTACHED_CONTEXT_THREAD_RECORD_OFFSET as u64;
        let record = unsafe { std::ptr::with_exposed_provenance::<u64>(address as usize).read() };
        assert_eq!(record, std::ptr::from_ref(py.execution_tls()) as u64);
        published_poll_word(record)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn lease_custody_binds_the_resolved_state_until_its_release() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        std::thread::spawn(|| {
            let tls = crate::state::execution_tls();
            assert!(tls.bound_state.get().is_null());
            assert_eq!(published_poll_word(std::ptr::from_ref(tls) as u64), 0);
            {
                let guard = RuntimeExecutionGuard::enter_with_worker_cleanup();
                let bound = tls.bound_state.get();
                let resolved = crate::state::runtime_state::runtime_state_for_gil_in(tls)
                    .expect("an admitted thread resolves its runtime");
                assert!(std::ptr::eq(bound, resolved));
                assert!(std::ptr::eq(crate::runtime_state(guard.token()), resolved));
                assert_eq!(
                    compiled_poll_word(guard.token()),
                    resolved.pending_work.word_ptr() as u64,
                    "a fresh callback must publish the real runtime eval breaker before any host entry"
                );
            }
            assert!(
                tls.bound_state.get().is_null(),
                "the last lease release resets the binding"
            );
            assert_eq!(published_poll_word(std::ptr::from_ref(tls) as u64), 0);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_runtime_switch_under_custody_rebinds_and_a_clear_unbinds() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        std::thread::spawn(|| {
            let tls = crate::state::execution_tls();
            let guard = RuntimeExecutionGuard::enter_with_worker_cleanup();
            let shared = crate::runtime_state(guard.token());
            assert_eq!(
                compiled_poll_word(guard.token()),
                shared.pending_work.word_ptr() as u64
            );
            let mut isolate = Box::new(crate::state::runtime_state::RuntimeState::new());
            crate::state::runtime_state::set_thread_runtime_state(&mut *isolate);
            assert!(std::ptr::eq(tls.bound_state.get(), &*isolate));
            assert!(std::ptr::eq(crate::runtime_state(guard.token()), &*isolate));
            assert_eq!(
                compiled_poll_word(guard.token()),
                isolate.pending_work.word_ptr() as u64
            );
            crate::state::runtime_state::clear_thread_runtime_state();
            assert!(tls.bound_state.get().is_null());
            assert_eq!(published_poll_word(std::ptr::from_ref(tls) as u64), 0);
            assert!(std::ptr::eq(crate::runtime_state(guard.token()), shared));
            // Internal full resolution after clear is distinct from publishing
            // a new compiled entry. The host boundary uses the same binder.
            assert_eq!(
                crate::state::tls::molt_attached_context(),
                guard.token().attached_context()
            );
            assert_eq!(
                compiled_poll_word(guard.token()),
                shared.pending_work.word_ptr() as u64
            );
            drop(guard);
            drop(isolate);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn drain_custody_binds_and_its_drop_unbinds() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        std::thread::spawn(|| {
            let tls = crate::state::execution_tls();
            let gil = GilGuard::new();
            let custody = ShutdownDrainExecutionCustody::enter();
            let resolved = crate::state::runtime_state::runtime_state_for_gil_in(tls)
                .expect("drain custody resolves the ready runtime");
            assert!(std::ptr::eq(tls.bound_state.get(), resolved));
            assert_eq!(
                compiled_poll_word(gil.token()),
                resolved.pending_work.word_ptr() as u64
            );
            {
                let nested = RuntimeExecutionGuard::enter();
                assert!(nested.owned.is_none(), "a callback borrows drain custody");
            }
            drop(custody);
            assert!(tls.bound_state.get().is_null());
            assert_eq!(published_poll_word(std::ptr::from_ref(tls) as u64), 0);
            drop(gil);
        })
        .join()
        .unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn suspended_gil_entry_inherits_ordinary_and_shutdown_custody() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        for shutdown in [false, true] {
            let gil = GilGuard::new();
            let ordinary = (!shutdown).then(RuntimeExecutionGuard::enter_with_worker_cleanup);
            let terminal = shutdown
                .then(ShutdownDrainExecutionCustody::enter_if_needed)
                .flatten();
            assert_eq!(terminal.is_some(), shutdown);
            let count = crate::state::runtime_state::active_runtime_execution_lease_count();
            let attached = molt_cpython_abi::api::object::runtime_execution_thread_is_attached();
            let runtime = crate::runtime_state(gil.token());
            let word = compiled_poll_word(gil.token());
            assert_eq!(word, runtime.pending_work.word_ptr() as u64);
            {
                let _release = crate::GilReleaseGuard::suspend();
                assert!(!crate::gil_held());
                let token = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
                assert!(crate::gil_held());
                assert!(std::ptr::eq(crate::runtime_state(gil.token()), runtime));
                assert_eq!(compiled_poll_word(gil.token()), word);
                assert!(
                    crate::obj_from_bits(crate::with_gil_entry_nopanic!(__entry_py, {
                        crate::object::ops_sys::molt_getrecursionlimit_attached_body(__entry_py)
                    }))
                    .as_int()
                    .unwrap()
                        > 0
                );
                assert_eq!(
                    crate::state::runtime_state::active_runtime_execution_lease_count(),
                    count
                );
                unsafe { molt_cpython_abi::api::object::PyGILState_Release(token) };
                assert!(!crate::gil_held());
                assert_eq!(
                    molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
                    attached
                );
                assert_eq!(
                    crate::state::runtime_state::active_runtime_execution_lease_count(),
                    count
                );
            }
            assert!(std::ptr::eq(crate::runtime_state(gil.token()), runtime));
            assert_eq!(compiled_poll_word(gil.token()), word);
            drop(terminal);
            drop(ordinary);
            drop(gil);
        }
    }

    #[test]
    fn shutdown_custody_inherits_only_real_lifetime_capabilities() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        {
            let _execution = RuntimeExecutionGuard::enter();
            assert!(ShutdownDrainExecutionCustody::enter_if_needed().is_none());
            assert!(!current_thread_holds_shutdown_drain_custody());
        }
        let _gil = GilGuard::new();
        // Bootstrap/finalizing C context is not ordinary admission.
        enter_c_extension_execution_context();
        let custody = ShutdownDrainExecutionCustody::enter_if_needed()
            .expect("C context without a lease must acquire shutdown custody");
        assert!(current_thread_holds_shutdown_drain_custody());
        assert!(ShutdownDrainExecutionCustody::enter_if_needed().is_none());
        {
            let nested = RuntimeExecutionGuard::enter();
            assert!(
                nested.owned.is_none(),
                "a nested entry borrows drain custody and the GIL"
            );
        }
        drop(custody);
        assert!(!current_thread_holds_shutdown_drain_custody());
        assert!(current_thread_has_c_extension_execution_context());
        leave_c_extension_execution_context();
        assert!(!current_thread_has_c_extension_execution_context());
    }

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
        let token = RuntimeExecutionGuard::enter_with_worker_cleanup().into_encoded_lane();
        let guard = unsafe { RuntimeExecutionGuard::from_encoded_lane(token) };
        assert!(current_thread_has_c_extension_execution_context());
        assert!(
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "scoped execution guard must attach before detach-panic proof"
        );
        RUNTIME_EXECUTION_DETACH_TEST_PANIC.with(|panic_next| panic_next.set(true));
        let outcome = crate::test_support::catch_expected_unwind(|| drop(guard));
        assert!(outcome.is_err());
        #[cfg(target_arch = "wasm32")]
        assert!(
            !execution_is_nested(),
            "detach panic must retire logical WASM depth"
        );
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
        let profile = crate::object::ops::runtime_profile_payload(gil.token(), false);
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
    fn wasm_shutdown_custody_owns_c_state_without_an_execution_frame() {
        use molt_cpython_abi::hooks::{AttachedRuntimeContextKind, hooks_or_stubs};

        let _test = crate::test_support::RuntimeTestTransaction::new();
        // The shared test setup now legitimately creates C state. Exercise
        // teardown's absent-record owner admission explicitly.
        drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
        let attached_context = || unsafe { (hooks_or_stubs().attached_runtime_context)() };
        let lease_count = crate::state::runtime_state::active_runtime_execution_lease_count();
        let retained_count = molt_cpython_abi::api::object::runtime_retained_thread_state_count();
        assert!(crate::state::runtime_state::runtime_is_initialized());
        assert!(!current_thread_has_runtime_execution_custody());
        assert!(!execution_is_nested());
        assert!(!crate::state::runtime_state::current_thread_holds_runtime_execution_lease());
        assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        assert_eq!(
            attached_context(),
            AttachedRuntimeContextKind::Detached as u32
        );

        {
            let _shutdown = ShutdownDrainExecutionCustody::enter();
            let mut owner = molt_cpython_abi::api::object::RuntimeShutdownThreadState::begin(
                crate::runtime_state(&GilGuard::new().token()).identity(),
            );
            assert!(current_thread_has_runtime_execution_custody());
            assert!(current_thread_has_c_extension_execution_context());
            assert!(!execution_is_nested());
            assert_eq!(
                attached_context(),
                AttachedRuntimeContextKind::WasmSingleThread as u32
            );
            {
                let nested = RuntimeExecutionGuard::enter();
                assert!(nested.owned.is_none(), "callback inherits shutdown custody");
                assert!(
                    !execution_is_nested(),
                    "shutdown callback is not an ordinary WASM frame"
                );
                assert!(
                    !crate::state::runtime_state::current_thread_holds_runtime_execution_lease()
                );
                assert_eq!(
                    crate::state::runtime_state::active_runtime_execution_lease_count(),
                    lease_count
                );
                assert!(molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
                assert_eq!(
                    molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                    retained_count + 1
                );
                assert_eq!(
                    attached_context(),
                    AttachedRuntimeContextKind::WasmSingleThread as u32
                );
            }
            assert!(current_thread_has_runtime_execution_custody());
            assert!(!execution_is_nested());
            assert_eq!(
                attached_context(),
                AttachedRuntimeContextKind::WasmSingleThread as u32
            );
            assert!(
                !unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null()
            );
            molt_cpython_abi::api::object::detach_runtime_execution_thread();
            let _drained = owner.drain(|| false);
            owner.finish();
        }

        assert!(!current_thread_has_runtime_execution_custody());
        assert!(!current_thread_has_c_extension_execution_context());
        assert!(!execution_is_nested());
        assert!(!crate::state::runtime_state::current_thread_holds_runtime_execution_lease());
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_count
        );
        assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            retained_count
        );
        assert_eq!(
            attached_context(),
            AttachedRuntimeContextKind::Detached as u32
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_logical_gil_and_execution_nesting_are_distinct() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        assert!(gil_held(), "single-threaded wasm owns the logical GIL");
        assert!(
            !execution_is_nested(),
            "logical always-held GIL must not fabricate an execution frame"
        );
        assert!(!current_thread_has_runtime_execution_custody());
        let outer_token = RuntimeExecutionGuard::enter().into_encoded_lane();
        let outer = unsafe { RuntimeExecutionGuard::from_encoded_lane(outer_token) };
        let record = unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() };
        assert!(!record.is_null());
        assert!(execution_is_nested());
        assert!(current_thread_has_runtime_execution_custody());
        let inner_token = RuntimeExecutionGuard::enter().into_encoded_lane();
        let inner = unsafe { RuntimeExecutionGuard::from_encoded_lane(inner_token) };
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() },
            record
        );
        drop(inner);
        assert!(
            execution_is_nested(),
            "nested exit must preserve the outer wasm execution frame"
        );
        drop(outer);
        assert!(!execution_is_nested());
        assert!(!current_thread_has_runtime_execution_custody());
        assert!(molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        let next = RuntimeExecutionGuard::enter_with_worker_cleanup();
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() },
            record
        );
        drop(next);
        assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
    }
}
