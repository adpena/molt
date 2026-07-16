#[cfg(not(target_arch = "wasm32"))]
use parking_lot::lock_api::RawMutex as _;
#[cfg(not(target_arch = "wasm32"))]
use parking_lot::{Mutex, MutexGuard};
#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;

#[cfg(not(target_arch = "wasm32"))]
use crate::GIL_DEPTH;

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

#[cfg(target_arch = "wasm32")]
pub(crate) struct PyToken<'gil> {
    _guard: &'gil GilGuard,
}

#[cfg(target_arch = "wasm32")]
impl GilGuard {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        let () = WASM_SINGLE_THREAD_GIL_CAPABILITY;
        Self {
            _not_send_sync: std::marker::PhantomData,
        }
    }

    #[inline(always)]
    pub(crate) fn token(&self) -> PyToken<'_> {
        PyToken { _guard: self }
    }

    #[inline(always)]
    pub(crate) fn new_extension_call() -> Self {
        Self::new()
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
pub(crate) fn with_gil<F, R>(f: F) -> R
where
    F: for<'gil> FnOnce(PyToken<'gil>) -> R,
{
    let guard = GilGuard::new();
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

/// Which of the three structurally-distinct acquisition lanes produced a
/// [`GilGuard`].  The lane determines what `Drop` must undo, so encoding it as
/// an explicit enum makes the three states mutually exclusive (no contradictory
/// `bool` combinations) and keeps every release obligation explicit.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
#[repr(u8)]
enum GilGuardLane {
    /// Normal TLS-backed custody. Every live guard owns one depth unit, while
    /// the mutex guard itself lives in `GIL_GUARD`. Guards may be dropped in
    /// any order; the mutex remains locked until the final 1->0 transition.
    Main = 1,
    /// The guard's depth unit was transferred into a `GilReleaseGuard` while
    /// custody is suspended. Drop has no remaining release obligation.
    Transferred = 2,
    /// TLS-destruction custody: exact owner and nesting live in synchronized
    /// process state because thread-local storage is no longer reachable.
    TlsDestruction = 3,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct GilGuard {
    lane: GilGuardLane,
    // GIL depth and raw TLS-destruction custody are thread-affine. Encoding that in
    // the type prevents a live guard (and its PyToken) from crossing threads.
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct PyToken<'gil> {
    _guard: &'gil GilGuard,
}

#[cfg(not(target_arch = "wasm32"))]
impl GilGuard {
    pub(crate) fn new() -> Self {
        match GIL_DEPTH.try_with(|depth| {
            let current = depth.get();
            if current == 0 {
                false
            } else {
                depth.set(current.checked_add(1).expect("GIL nesting depth overflow"));
                true
            }
        }) {
            Ok(true) => {
                return Self {
                    lane: GilGuardLane::Main,
                    _not_send_sync: std::marker::PhantomData,
                };
            }
            Ok(false) => {}
            Err(_) => return Self::tls_destruction_new(),
        }

        let guard = molt_gil().lock();
        if GIL_DEPTH.try_with(|depth| depth.set(1)).is_err() {
            drop(guard);
            return Self::tls_destruction_new();
        }
        let stored = GIL_GUARD
            .try_with(|slot| {
                *slot.borrow_mut() = Some(guard);
            })
            .is_ok();
        if !stored {
            let _ = GIL_DEPTH.try_with(|depth| depth.set(0));
            return Self::tls_destruction_new();
        }
        Self {
            lane: GilGuardLane::Main,
            _not_send_sync: std::marker::PhantomData,
        }
    }

    #[inline(always)]
    pub(crate) fn new_extension_call() -> Self {
        Self::new()
    }

    pub(crate) fn token(&self) -> PyToken<'_> {
        PyToken { _guard: self }
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
            value if value == GilGuardLane::TlsDestruction as u64 => GilGuardLane::TlsDestruction,
            _ => panic!("invalid encoded GIL custody lane {token}"),
        };
        Self {
            lane,
            _not_send_sync: std::marker::PhantomData,
        }
    }

    fn tls_destruction_new() -> Self {
        let owner = std::thread::current().id();
        {
            let mut state = TLS_DESTRUCTION_GIL_STATE.lock().unwrap();
            if state.owner == Some(owner) {
                state.depth = state
                    .depth
                    .checked_add(1)
                    .expect("TLS-destruction GIL nesting depth overflow");
                return Self {
                    lane: GilGuardLane::TlsDestruction,
                    _not_send_sync: std::marker::PhantomData,
                };
            }
        }

        // TLS is unavailable, so custody cannot live in a thread-local guard.
        // Lock the raw mutex and publish exact ThreadId/depth metadata in one
        // synchronized process state; whichever destruction token drops last owns
        // the raw unlock, independent of token drop order.
        unsafe {
            molt_gil().raw().lock();
        }
        let mut state = TLS_DESTRUCTION_GIL_STATE.lock().unwrap();
        assert!(state.owner.is_none());
        assert_eq!(state.depth, 0);
        state.owner = Some(owner);
        state.depth = 1;
        Self {
            lane: GilGuardLane::TlsDestruction,
            _not_send_sync: std::marker::PhantomData,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for GilGuard {
    fn drop(&mut self) {
        match self.lane {
            GilGuardLane::Transferred => {}
            GilGuardLane::TlsDestruction => {
                let should_unlock = {
                    let mut state = TLS_DESTRUCTION_GIL_STATE.lock().unwrap();
                    assert_eq!(state.owner, Some(std::thread::current().id()));
                    assert!(state.depth > 0);
                    state.depth -= 1;
                    if state.depth == 0 {
                        state.owner = None;
                        true
                    } else {
                        false
                    }
                };
                if should_unlock {
                    unsafe {
                        molt_gil().force_unlock();
                    }
                }
            }
            GilGuardLane::Main => {
                let should_release = GIL_DEPTH
                    .try_with(|depth| {
                        let current = depth.get();
                        assert!(current > 0, "live GIL guard lost its custody depth");
                        let next = current - 1;
                        depth.set(next);
                        next == 0
                    })
                    .expect("main GIL custody reached Drop after TLS destruction");
                if should_release {
                    let guard = GIL_GUARD
                        .try_with(|slot| slot.borrow_mut().take())
                        .expect("GIL mutex custody TLS disappeared before final release")
                        .expect("final GIL depth unit had no mutex custody");
                    MutexGuard::unlock_fair(guard);
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct GilReleaseGuard {
    depth: usize,
    had_runtime_guard: bool,
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl GilReleaseGuard {
    pub(crate) fn suspend() -> Self {
        let depth = GIL_DEPTH
            .try_with(|d| d.get())
            .expect("cannot release the GIL after custody TLS destruction");
        assert!(depth > 0, "cannot release a GIL this thread does not hold");
        GIL_DEPTH
            .try_with(|d| d.set(0))
            .expect("GIL custody TLS disappeared during release");
        let released = GIL_GUARD
            .try_with(|slot| slot.borrow_mut().take())
            .expect("GIL mutex custody TLS disappeared during release")
            .expect("positive GIL depth had no mutex custody");
        MutexGuard::unlock_fair(released);
        let runtime_guard = RUNTIME_GIL_GUARD
            .try_with(|slot| slot.borrow_mut().take())
            .expect("runtime GIL custody TLS disappeared during release");
        let had_runtime_guard = runtime_guard.is_some();
        // Its custody unit is included in `depth`; transfer that obligation to
        // this release guard, which creates the replacement token on restore.
        if let Some(guard) = runtime_guard {
            guard.transfer_custody_unit();
        }
        Self {
            depth,
            had_runtime_guard,
            _not_send_sync: std::marker::PhantomData,
        }
    }

    pub(crate) fn into_encoded_state(mut self) -> u64 {
        let depth = u64::try_from(self.depth).expect("GIL release depth exceeds ABI width");
        let token = depth
            .checked_mul(2)
            .expect("GIL release depth cannot be encoded")
            | u64::from(self.had_runtime_guard);
        self.depth = 0;
        self.had_runtime_guard = false;
        token
    }

    /// # Safety
    ///
    /// `token` must come from an unmatched `into_encoded_state` call on this
    /// thread and must be reconstructed exactly once.
    pub(crate) unsafe fn from_encoded_state(token: u64) -> Self {
        let depth = usize::try_from(token >> 1).expect("GIL release depth exceeds target width");
        let had_runtime_guard = (token & 1) != 0;
        assert!(
            depth > 0,
            "encoded GIL release state cannot represent absent custody"
        );
        Self {
            depth,
            had_runtime_guard,
            _not_send_sync: std::marker::PhantomData,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for GilReleaseGuard {
    fn drop(&mut self) {
        if self.depth == 0 {
            return;
        }
        if self.had_runtime_guard {
            hold_runtime_gil(GilGuard::new());
            GIL_DEPTH
                .try_with(|d| d.set(self.depth))
                .expect("GIL depth TLS disappeared during persistent restore");
            return;
        }
        let guard = molt_gil().lock();
        GIL_GUARD
            .try_with(|slot| {
                let mut slot = slot.borrow_mut();
                assert!(slot.is_none(), "GIL restore found live mutex custody");
                *slot = Some(guard);
            })
            .expect("GIL mutex custody TLS disappeared during restore");
        GIL_DEPTH
            .try_with(|d| d.set(self.depth))
            .expect("GIL depth TLS disappeared during restore");
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gil_held() -> bool {
    match GIL_DEPTH.try_with(|depth| depth.get()) {
        Ok(depth) => depth > 0 || tls_destruction_gil_held(),
        Err(_) => tls_destruction_gil_held(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gil_owned_by_current_thread() -> bool {
    match GIL_DEPTH.try_with(|depth| depth.get()) {
        Ok(depth) => depth > 0 || tls_destruction_gil_held(),
        Err(_) => tls_destruction_gil_held(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static GIL_GUARD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
    static RUNTIME_GIL_GUARD: RefCell<Option<GilGuard>> = const { RefCell::new(None) };
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn hold_runtime_gil(guard: GilGuard) {
    RUNTIME_GIL_GUARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        assert!(slot.is_none(), "runtime GIL custody already installed");
        *slot = Some(guard);
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn release_runtime_gil() {
    RUNTIME_GIL_GUARD.with(|slot| {
        drop(
            slot.borrow_mut()
                .take()
                .expect("runtime GIL release without installed custody"),
        );
    });
}

#[cfg(not(target_arch = "wasm32"))]
struct TlsDestructionGilState {
    owner: Option<std::thread::ThreadId>,
    depth: usize,
}

#[cfg(not(target_arch = "wasm32"))]
static TLS_DESTRUCTION_GIL_STATE: std::sync::Mutex<TlsDestructionGilState> =
    std::sync::Mutex::new(TlsDestructionGilState {
        owner: None,
        depth: 0,
    });

#[cfg(not(target_arch = "wasm32"))]
fn tls_destruction_gil_held() -> bool {
    let state = TLS_DESTRUCTION_GIL_STATE.lock().unwrap();
    state.owner == Some(std::thread::current().id()) && state.depth > 0
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn with_gil<F, R>(f: F) -> R
where
    F: for<'gil> FnOnce(PyToken<'gil>) -> R,
{
    let guard = GilGuard::new();
    let token = guard.token();
    f(token)
}

// ---------------------------------------------------------------------------
// gil_assert: available on both targets
// ---------------------------------------------------------------------------

#[cfg(feature = "molt_debug_gil")]
pub(crate) fn gil_assert() {
    assert!(gil_held(), "GIL required for runtime mutation");
}

#[cfg(not(feature = "molt_debug_gil"))]
pub(crate) fn gil_assert() {
    debug_assert!(gil_held(), "GIL required for runtime mutation");
}

// ---------------------------------------------------------------------------
// Tests (non-wasm32 only — they rely on threads and the mutex-based GIL)
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{GilGuard, gil_held};
    use crate::GIL_DEPTH;
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
        let start = GIL_DEPTH.with(|depth| depth.get());
        {
            // Outermost guard performs the 0->1 transition and takes the mutex.
            let _g1 = GilGuard::new();
            let depth1 = GIL_DEPTH.with(|depth| depth.get());
            assert_eq!(depth1, start + 1, "outer guard owns the depth gate");
            assert!(gil_held());
            {
                // Re-entry increments custody without relocking the mutex.
                let _g2 = GilGuard::new();
                let depth2 = GIL_DEPTH.with(|depth| depth.get());
                assert_eq!(depth2, depth1 + 1);
                assert!(gil_held());
                {
                    // Deeper nesting adds another independent custody unit.
                    let _g3 = GilGuard::new();
                    let depth3 = GIL_DEPTH.with(|depth| depth.get());
                    assert_eq!(depth3, depth2 + 1);
                    assert!(gil_held());
                }
                // Dropping the deepest guard leaves the other two live.
                assert_eq!(GIL_DEPTH.with(|depth| depth.get()), depth2);
                assert!(gil_held());
            }
            // Dropping inner guards leaves the outer gate untouched.
            let depth1_after = GIL_DEPTH.with(|depth| depth.get());
            assert_eq!(depth1_after, start + 1);
            assert!(gil_held());
        }

        // The outer guard's drop performed the 1->0 transition and released the
        // mutex exactly once.
        let final_depth = GIL_DEPTH.with(|depth| depth.get());
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
        assert_eq!(GIL_DEPTH.with(|depth| depth.get()), 2);
        proceed_tx.send(()).unwrap();
        drop(outer);
        assert_eq!(GIL_DEPTH.with(|depth| depth.get()), 1);
        assert!(gil_held());
        assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(inner);
        assert_eq!(GIL_DEPTH.with(|depth| depth.get()), 0);
        assert!(!gil_held());
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn releasing_persistent_custody_preserves_live_shutdown_guard() {
        super::hold_runtime_gil(GilGuard::new());
        let shutdown_guard = GilGuard::new();
        assert_eq!(GIL_DEPTH.with(|depth| depth.get()), 2);

        super::release_runtime_gil();
        assert_eq!(GIL_DEPTH.with(|depth| depth.get()), 1);
        assert!(gil_held());
        assert!(
            super::molt_gil().try_lock().is_none(),
            "dropping persistent custody must not unlock beneath shutdown"
        );

        drop(shutdown_guard);
        assert!(!gil_held());
    }

    #[test]
    fn tls_destruction_outer_drop_preserves_custody_until_last_guard() {
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

        let outer = GilGuard::tls_destruction_new();
        let inner = GilGuard::tls_destruction_new();
        assert!(super::tls_destruction_gil_held());
        proceed_tx.send(()).unwrap();
        drop(outer);
        assert!(super::tls_destruction_gil_held());
        assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(inner);
        assert!(!super::tls_destruction_gil_held());
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
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
}
