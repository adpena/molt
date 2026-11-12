#[cfg(not(target_arch = "wasm32"))]
use parking_lot::{Mutex, MutexGuard};
#[cfg(not(target_arch = "wasm32"))]
use std::cell::{Cell, RefCell};
#[cfg(not(target_arch = "wasm32"))]
use std::mem::ManuallyDrop;
#[cfg(not(target_arch = "wasm32"))]
use std::num::NonZeroU64;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(not(target_arch = "wasm32"))]
use crate::state::{ExecutionTls, execution_tls};

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
compile_error!(
    "threaded wasm requires a real shared GIL authority; the single-thread wasm GIL cannot be selected with target_feature=atomics"
);

#[cfg(target_arch = "wasm32")]
const WASM_SINGLE_THREAD_GIL_CAPABILITY: () = ();

// ---------------------------------------------------------------------------
// wasm32: single-threaded target — the GIL is always held, all operations
// are no-ops.  We keep the public types so call-sites compile unchanged.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub(crate) struct GilGuard {
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

// Releasing the guard is part of its contract on every target: call sites
// drop it at the exact point the GIL must be given up. On this single-threaded
// target the release does nothing, but the drop stays the observable release
// point so the same code lowers on wasm32 and on native.
#[cfg(target_arch = "wasm32")]
impl Drop for GilGuard {
    #[inline(always)]
    fn drop(&mut self) {}
}

/// Proof that this thread holds the GIL for `'gil`, carrying its execution
/// record. A GIL guard or a runtime execution entry produces it; a nested
/// entry that borrows its caller's custody produces the same type.
#[repr(transparent)]
pub(crate) struct PyToken<'gil> {
    core: molt_runtime_core::PyToken,
    _custody: std::marker::PhantomData<&'gil ()>,
}

impl<'gil> PyToken<'gil> {
    /// # Safety
    /// This is the caller's permanent execution record and its GIL custody
    /// remains live for the complete returned borrow.
    #[inline(always)]
    pub(crate) unsafe fn with_custody(record: &'static crate::state::ExecutionTls) -> &'gil Self {
        record
            .attached_context
            .bind_thread_record(std::ptr::from_ref(record) as u64);
        let context = std::ptr::from_ref(&record.attached_context) as u64;
        unsafe { Self::from_attached_context(context) }
    }

    /// # Safety
    /// The generated ABI's caller retains this exact attached context through
    /// return, including balanced suspension/callback scopes.
    #[inline(always)]
    pub(crate) unsafe fn from_attached_context(context: u64) -> &'gil Self {
        let core = unsafe { molt_runtime_core::PyToken::from_attached_context(context) };
        unsafe { &*std::ptr::from_ref(core).cast::<Self>() }
    }

    #[inline(always)]
    pub(crate) fn core_token(&self) -> &molt_runtime_core::PyToken {
        &self.core
    }

    #[inline(always)]
    pub(crate) fn attached_context(&self) -> u64 {
        self.core.attached_context()
    }

    #[inline(always)]
    pub(crate) fn execution_tls(&self) -> &'static crate::state::ExecutionTls {
        unsafe { &*std::ptr::with_exposed_provenance(self.core.thread_record_address() as usize) }
    }
}

#[cfg(target_arch = "wasm32")]
impl GilGuard {
    #[inline(always)]
    pub(crate) fn new_if_held_in(_tls: &'static crate::state::ExecutionTls) -> Option<Self> {
        Some(Self::new())
    }

    #[inline(always)]
    pub(crate) fn new() -> Self {
        let () = WASM_SINGLE_THREAD_GIL_CAPABILITY;
        Self {
            _not_send_sync: std::marker::PhantomData,
        }
    }

    #[inline(always)]
    pub(crate) fn token(&self) -> &PyToken<'_> {
        // SAFETY: this guard is the runtime's actual GIL custody boundary.
        unsafe { PyToken::with_custody(crate::state::execution_tls()) }
    }

    #[inline(always)]
    pub(crate) fn into_encoded_lane(self) -> u64 {
        1
    }

    #[inline(always)]
    /// # Safety
    ///
    /// `token` must come from an unmatched `into_encoded_lane` call on this
    /// thread and must be reconstructed exactly once.
    pub(crate) unsafe fn from_encoded_lane(token: u64) -> Self {
        assert_eq!(token, 1, "invalid wasm GIL custody lane {token}");
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct GilReleaseGuard {
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

// Dropping the release guard re-acquires the GIL on native; here it is the
// same no-op release point, kept so call sites stay target-independent.
#[cfg(target_arch = "wasm32")]
impl Drop for GilReleaseGuard {
    #[inline(always)]
    fn drop(&mut self) {}
}

#[cfg(target_arch = "wasm32")]
impl GilReleaseGuard {
    #[inline(always)]
    pub(crate) fn suspend() -> Self {
        Self {
            _not_send_sync: std::marker::PhantomData,
        }
    }

    #[inline(always)]
    pub(crate) fn into_encoded_state(self) -> u64 {
        0
    }

    /// # Safety
    ///
    /// `token` must come from an unmatched `into_encoded_state` call in the
    /// current execution context and must be reconstructed exactly once.
    pub(crate) unsafe fn from_encoded_state(token: u64) -> Self {
        assert_eq!(token, 0, "invalid wasm GIL release state {token}");
        Self::suspend()
    }
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn gil_held() -> bool {
    // On wasm32 the GIL is logically always held (single-threaded).
    true
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn gil_owned_by_current_thread() -> bool {
    true
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn hold_runtime_gil(_guard: GilGuard) {
    // no-op
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn release_runtime_gil() {
    // no-op
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
pub(crate) fn touch_gil_tls_lifetime() {
    // wasm has no native thread-local GIL custody.
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn with_gil<F, R>(f: F) -> R
where
    F: for<'gil> FnOnce(&'gil PyToken<'gil>) -> R,
{
    let guard = crate::concurrency::RuntimeExecutionGuard::enter();
    let token = guard.token();
    f(token)
}

// ---------------------------------------------------------------------------
// Non-wasm32: full mutex-based GIL implementation
// ---------------------------------------------------------------------------

/// The single global GIL mutex.
///
/// This is always a `'static` mutex shared across all phases of the runtime
/// lifetime: pre-init, active, and post-shutdown.  Earlier designs returned
/// `&state.gil` once the runtime state existed and `&GLOBAL_GIL` otherwise,
/// but that produced a synchronization gap: a thread that acquired the GIL
/// before init (via `GLOBAL_GIL`) and another thread that acquired it after
/// init (via `state.gil`) were taking *different* mutexes, so neither
/// happens-before-synchronized with the other.  Miri's data-race detector
/// caught this in the `builtins::modules::tests` cross-test interaction:
/// two test threads concurrently mutated `sys.modules`'s order Vec because
/// the mutex they each held was distinct.
///
/// Keeping a single static mutex eliminates that gap entirely while still
/// surviving `molt_runtime_shutdown` (the static is `'static` and is never
/// dropped, unlike a mutex stored in the heap-allocated runtime state).
#[cfg(not(target_arch = "wasm32"))]
static GLOBAL_GIL: Mutex<()> = Mutex::new(());

#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn molt_gil() -> &'static Mutex<()> {
    &GLOBAL_GIL
}

/// Take the GIL as CPython's `take_gil` does. Compiled code holds the GIL
/// for as long as it runs and gives it up only around blocking calls, so a
/// waiter that is not served within the switch interval sets the drop
/// request in the runtime's eval breaker. The holder's next generated poll
/// services it (`yield_gil_to_waiter`) and `unlock_fair` hands the mutex to
/// the queued waiter. Without the request a loop that never blocks starves
/// every other thread.
#[cfg(not(target_arch = "wasm32"))]
fn take_gil(tls: &ExecutionTls) -> MutexGuard<'static, ()> {
    let gil = molt_gil();
    if let Some(guard) = gil.try_lock() {
        return guard;
    }
    let interval = crate::builtins::sys_ext::gil_switch_interval(tls.bound_state.get());
    loop {
        if let Some(guard) = gil.try_lock_for(interval) {
            return guard;
        }
        crate::builtins::signal_ext::notify_gil_drop_request();
    }
}

/// The holder's half of a drop request, at a generated poll: release the
/// GIL to the waiter that asked and take it back in turn.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn yield_gil_to_waiter() {
    drop(GilReleaseGuard::suspend());
}

/// Single-threaded wasm has no waiter; no producer sets the request there.
#[cfg(target_arch = "wasm32")]
pub(crate) fn yield_gil_to_waiter() {}

/// Whether this guard still owns its normal depth unit or transferred it.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
#[repr(u8)]
enum GilGuardLane {
    /// Every live guard owns one depth unit, while the mutex acquisition lives
    /// in the destructor-free execution record. Guards may be dropped in
    /// any order; the mutex remains locked until the final 1->0 transition.
    Main = 1,
    /// The guard's depth unit was transferred into a `GilReleaseGuard` while
    /// custody is suspended. Drop has no remaining release obligation.
    Transferred = 2,
}

/// One unmatched suspend/restore transition, unique across native threads.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GilSuspensionId(NonZeroU64);

#[cfg(not(target_arch = "wasm32"))]
static NEXT_GIL_SUSPENSION: AtomicU64 = AtomicU64::new(1);

#[cfg(not(target_arch = "wasm32"))]
impl GilSuspensionId {
    fn new() -> Self {
        let value = NEXT_GIL_SUSPENSION
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("GIL suspension identity space exhausted");
        Self(NonZeroU64::new(value).expect("GIL suspension identity is nonzero"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct SavedGilCustody {
    id: GilSuspensionId,
    depth: usize,
    persistent: bool,
    runtime: *const crate::state::runtime_state::RuntimeState,
}

/// Initialized, explicitly consumed ownership in the thread's permanent record.
/// ManuallyDrop suppresses only automatic TLS destruction: the RefCells remain
/// valid, and Option::take/Vec::pop move their owned values safely. No borrow
/// survives an unlock, callback or destructor of a mutex acquisition.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct GilMutexCustody {
    acquisition: ManuallyDrop<RefCell<Option<MutexGuard<'static, ()>>>>,
    saved: ManuallyDrop<RefCell<Vec<SavedGilCustody>>>,
    persistent: Cell<bool>,
    lifetime_retired: Cell<bool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl GilMutexCustody {
    pub(crate) const fn new() -> Self {
        Self {
            acquisition: ManuallyDrop::new(RefCell::new(None)),
            saved: ManuallyDrop::new(RefCell::new(Vec::new())),
            persistent: Cell::new(false),
            lifetime_retired: Cell::new(false),
        }
    }

    fn install(&self, acquisition: MutexGuard<'static, ()>) {
        let mut slot = self.acquisition.borrow_mut();
        assert!(
            slot.is_none(),
            "GIL acquisition overwrote live mutex custody"
        );
        *slot = Some(acquisition);
    }

    fn take(&self) -> MutexGuard<'static, ()> {
        self.acquisition
            .borrow_mut()
            .take()
            .expect("positive GIL depth had no mutex custody")
    }

    pub(crate) fn suspension_identity(&self) -> u64 {
        self.saved
            .borrow()
            .last()
            .map_or(0, |saved| saved.id.0.get())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct GilGuard {
    lane: GilGuardLane,
    /// This thread's execution facts, read once at entry and reused on drop.
    tls: &'static ExecutionTls,
    // GIL depth and mutex custody are thread-affine. Encoding that in
    // the type prevents a live guard (and its PyToken) from crossing threads.
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl GilGuard {
    /// The nested guard of a thread that already holds the GIL, read from
    /// the execution facts its entry has already loaded.
    #[inline(always)]
    pub(crate) fn new_if_held_in(tls: &'static ExecutionTls) -> Option<Self> {
        let current = tls.gil_depth.get();
        if current != 0 {
            tls.gil_depth
                .set(current.checked_add(1).expect("GIL nesting depth overflow"));
            return Some(Self {
                lane: GilGuardLane::Main,
                tls,
                _not_send_sync: std::marker::PhantomData,
            });
        }
        None
    }

    pub(crate) fn new() -> Self {
        let tls = execution_tls();
        if let Some(nested) = Self::new_if_held_in(tls) {
            return nested;
        }

        let guard = take_gil(tls);
        tls.native_gil.install(guard);
        tls.gil_depth.set(1);
        Self {
            lane: GilGuardLane::Main,
            tls,
            _not_send_sync: std::marker::PhantomData,
        }
    }

    #[inline(always)]
    pub(crate) fn token(&self) -> &PyToken<'_> {
        // SAFETY: this guard is the runtime's actual GIL custody boundary.
        unsafe { PyToken::with_custody(self.tls) }
    }

    fn transfer_custody_unit(mut self) {
        debug_assert!(matches!(self.lane, GilGuardLane::Main));
        self.lane = GilGuardLane::Transferred;
    }

    pub(crate) fn into_encoded_lane(mut self) -> u64 {
        let token = self.lane as u64;
        assert_ne!(self.lane as u8, GilGuardLane::Transferred as u8);
        self.lane = GilGuardLane::Transferred;
        token
    }

    /// # Safety
    ///
    /// `token` must come from an unmatched `into_encoded_lane` call on this
    /// thread and must be reconstructed exactly once.
    pub(crate) unsafe fn from_encoded_lane(token: u64) -> Self {
        let lane = match token {
            value if value == GilGuardLane::Main as u64 => GilGuardLane::Main,
            _ => panic!("invalid encoded GIL custody lane {token}"),
        };
        Self {
            lane,
            tls: execution_tls(),
            _not_send_sync: std::marker::PhantomData,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for GilGuard {
    #[inline(always)]
    fn drop(&mut self) {
        // The nested guard of a runtime entry only gives back its depth unit.
        if let GilGuardLane::Main = self.lane {
            let current = self.tls.gil_depth.get();
            if current > 1 {
                self.tls.gil_depth.set(current - 1);
                return;
            }
        }
        self.drop_cold();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl GilGuard {
    #[cold]
    #[inline(never)]
    fn drop_cold(&mut self) {
        match self.lane {
            GilGuardLane::Transferred => {}
            GilGuardLane::Main => {
                let current = self.tls.gil_depth.get();
                assert!(current > 0, "live GIL guard lost its custody depth");
                self.tls.gil_depth.set(current - 1);
                if current == 1 {
                    let guard = self.tls.native_gil.take();
                    MutexGuard::unlock_fair(guard);
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct GilReleaseGuard {
    tls: &'static ExecutionTls,
    id: Option<GilSuspensionId>,
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl GilReleaseGuard {
    pub(crate) fn suspend() -> Self {
        let tls = execution_tls();
        let depth = tls.gil_depth.get();
        assert!(depth > 0, "cannot release a GIL this thread does not hold");
        let id = GilSuspensionId::new();
        // Reserve before changing custody: allocation failure cannot strand an
        // acquired GIL. The stack is an ownership protocol, not a lookup cache.
        let mut saved = tls.native_gil.saved.borrow_mut();
        saved.reserve(1);
        saved.push(SavedGilCustody {
            id,
            depth,
            persistent: tls.native_gil.persistent.replace(false),
            runtime: tls.bound_state.get(),
        });
        drop(saved);
        tls.gil_depth.set(0);
        let released = tls.native_gil.take();
        MutexGuard::unlock_fair(released);
        Self {
            tls,
            id: Some(id),
            _not_send_sync: std::marker::PhantomData,
        }
    }

    pub(crate) fn into_encoded_state(mut self) -> u64 {
        self.id
            .take()
            .expect("GIL suspension transferred twice")
            .0
            .get()
    }

    /// # Safety
    ///
    /// `token` must come from an unmatched `into_encoded_state` call on this
    /// thread and must be reconstructed exactly once.
    pub(crate) unsafe fn from_encoded_state(token: u64) -> Self {
        let id = GilSuspensionId(NonZeroU64::new(token).expect("missing GIL suspension identity"));
        let tls = execution_tls();
        assert_eq!(
            tls.native_gil.suspension_identity(),
            token,
            "GIL restore crossed suspension ownership"
        );
        Self {
            tls,
            id: Some(id),
            _not_send_sync: std::marker::PhantomData,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for GilReleaseGuard {
    fn drop(&mut self) {
        let Some(id) = self.id else {
            return;
        };
        let tls = self.tls;
        let saved = *tls
            .native_gil
            .saved
            .borrow()
            .last()
            .expect("GIL restore lost its saved frame");
        assert_eq!(
            saved.id, id,
            "GIL restoration must close the innermost suspension"
        );
        assert_eq!(
            tls.gil_depth.get(),
            0,
            "GIL restore crossed live callback custody"
        );
        assert_eq!(
            tls.bound_state.get(),
            saved.runtime,
            "GIL restore crossed runtime identity"
        );
        assert!(
            !tls.native_gil.persistent.get(),
            "GIL restore found another persistent owner"
        );
        let guard = take_gil(tls);
        tls.native_gil.install(guard);
        tls.gil_depth.set(saved.depth);
        tls.native_gil.persistent.set(saved.persistent);
        let retired_storage = {
            let mut stack = tls.native_gil.saved.borrow_mut();
            let popped = stack.pop().expect("GIL restore lost its saved frame");
            assert_eq!(popped.id, id);
            (stack.is_empty() && tls.native_gil.lifetime_retired.get())
                .then(|| std::mem::take(&mut *stack))
        };
        // Late TLS finalizers can suspend after the lifetime sentinel retired.
        // Release their empty stack capacity explicitly on their last restore.
        drop(retired_storage);
        self.id = None;
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gil_held() -> bool {
    execution_tls().gil_depth.get() > 0
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gil_owned_by_current_thread() -> bool {
    execution_tls().gil_depth.get() > 0
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static GIL_LIFETIME: GilLifetime = const { GilLifetime };
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn hold_runtime_gil(guard: GilGuard) {
    touch_gil_tls_lifetime();
    assert!(
        !guard.tls.native_gil.persistent.replace(true),
        "runtime GIL custody already installed"
    );
    guard.transfer_custody_unit();
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn release_runtime_gil() {
    let tls = execution_tls();
    assert!(
        tls.native_gil.persistent.replace(false),
        "runtime GIL release without installed custody"
    );
    // SAFETY: persistent owns exactly one transferred, same-thread depth unit.
    drop(unsafe { GilGuard::from_encoded_lane(GilGuardLane::Main as u64) });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn touch_gil_tls_lifetime() {
    let _ = GIL_LIFETIME.try_with(|_| {});
}

#[cfg(not(target_arch = "wasm32"))]
struct GilLifetime;

#[cfg(not(target_arch = "wasm32"))]
impl Drop for GilLifetime {
    fn drop(&mut self) {
        let tls = execution_tls();
        assert!(
            tls.native_gil.saved.borrow().is_empty(),
            "thread exit crossed suspended GIL custody"
        );
        if tls.native_gil.persistent.get() {
            release_runtime_gil();
        }
        assert_eq!(
            tls.gil_depth.get(),
            0,
            "thread exit crossed live GIL custody"
        );
        assert!(tls.native_gil.acquisition.borrow().is_none());
        tls.native_gil.lifetime_retired.set(true);
        crate::cpython_abi_hooks::clear_abi_gil_tls_storage();
        let storage = std::mem::take(&mut *tls.native_gil.saved.borrow_mut());
        drop(storage);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn with_gil<F, R>(f: F) -> R
where
    F: for<'gil> FnOnce(&'gil PyToken<'gil>) -> R,
{
    let guard = crate::concurrency::RuntimeExecutionGuard::enter();
    let token = guard.token();
    f(token)
}

// ---------------------------------------------------------------------------
// gil_assert: available on both targets
// ---------------------------------------------------------------------------

pub(crate) fn gil_assert() {
    debug_assert!(gil_held(), "GIL required for runtime mutation");
}

// ---------------------------------------------------------------------------
// Tests (non-wasm32 only — they rely on threads and the mutex-based GIL)
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "gil/tests.rs"]
mod tests;
