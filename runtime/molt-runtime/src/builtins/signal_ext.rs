#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/signal_ext.rs ===
//! Signal authority: the `signal`/`_signal` intrinsics, the raw OS handler,
//! Python-handler dispatch, and the CPython signal C-API projections.
//!
//! State is runtime-scoped in `RuntimeState.signal`: handler slots, pending
//! delivery flags, the Python-visible wakeup fd, and the park route of the
//! event loop parked on the registered main thread. A handler slot holds
//! SIG_DFL (0), SIG_IGN (1), SIGINT's startup `default_int_handler`
//! disposition (2), or retained callable bits.
//!
//! Every delivery — a raw OS signal, `PyErr_SetInterruptEx`,
//! `_thread.interrupt_main`, or a `raise_signal` with no OS handler behind it —
//! only records: pending flag, then the tripped summary, then wake bytes
//! (CPython `trip_signal`; bpo-30038). Python handlers run only on the thread
//! registered as process main at runtime initialization (the pending-call
//! authority's identity), in signal-number order, at:
//! - the generated eval-breaker safepoint (`service_async_work`), before
//!   pending calls, as CPython's `_Py_HandlePending` does;
//! - `signal.signal` (before replacing a handler), `raise_signal` and `pause`;
//! - C `PyErr_CheckSignals`.
//!
//! Anything a delivery can reach — the active state pointer, a published park
//! route, a retired wakeup fd — is withdrawn before it is retired, and the
//! retiring thread waits, without holding the GIL, until every delivery
//! admitted before the withdrawal has left (`ASYNC_WORK_NOTIFIERS_IN_FLIGHT`).
//!
//! SIGINT policy: `getsignal(SIGINT)` reports `default_int_handler` at startup
//! and simulated deliveries raise KeyboardInterrupt, but the OS disposition
//! stays SIG_DFL so a terminal Ctrl-C still ends a process blocked in a wait
//! that signals cannot yet interrupt. Installing any Python handler (asyncio's
//! Runner does) routes OS deliveries through the raw handler. A process whose
//! exact KeyboardInterrupt escaped exits by SIGINT after finalization
//! (`signal_exit_status_after_finalization`).
//!
//! ABI: NaN-boxed u64 in/out.

use crate::audit::AuditArgs;
use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

// ── Constants ─────────────────────────────────────────────────────────────

pub const SIG_DFL_INT: i64 = 0;
pub const SIG_IGN_INT: i64 = 1;

/// Sentinel bits stored in the handler table. A retained callable is always a
/// heap object (callability is checked at installation), so its NaN-boxed bits
/// never collide with these raw values.
const HANDLER_SIG_DFL: u64 = 0;
const HANDLER_SIG_IGN: u64 = 1;
/// SIGINT's startup disposition: Python-visible `default_int_handler` with the
/// OS default action (see the module SIGINT policy).
const HANDLER_DEFAULT_INT: u64 = 2;

const HANDLER_TYPE_ERROR: &str =
    "signal handler must be signal.SIG_IGN, signal.SIG_DFL, or a callable object";

/// Maximum signal slot count reserved by the runtime table.
///
/// We keep this slightly above common platform NSIG ranges so we can index
/// fixed-size atomics without heap allocations, while still validating user
/// signal numbers against platform NSIG at API boundaries.
const MAX_SIGNAL: usize = 128;

/// Whether this target delivers OS signals to an installed raw handler.
const OS_SIGNAL_DELIVERY: bool = cfg!(all(unix, not(target_arch = "wasm32")));

/// Lock protecting sigaction calls on Unix.
static SIGACTION_LOCK: Mutex<()> = Mutex::new(());

/// Signal handlers run without the GIL and cannot touch TLS or locks. This
/// pointer is only a signal-safe route to the currently active runtime-owned
/// atomics; `RuntimeState.signal` remains the owner.
static ACTIVE_SIGNAL_STATE: AtomicPtr<SignalRuntimeState> = AtomicPtr::new(std::ptr::null_mut());

/// Deliveries between admission and exit. Retiring anything a delivery can
/// reach withdraws it first, then waits for this to drain.
static ASYNC_WORK_NOTIFIERS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Summary of recorded, undispatched deliveries in the active state (CPython
/// `is_tripped`): the eval-breaker fast path is one relaxed load of it.
static SIGNALS_TRIPPED: AtomicBool = AtomicBool::new(false);

pub(crate) struct SignalRuntimeState {
    handlers: [AtomicU64; MAX_SIGNAL],
    wakeup_fd: AtomicI32,
    pending: [AtomicU64; MAX_SIGNAL],
    /// Parker of the event loop parked on the registered main thread, for
    /// exactly one park (`AsyncWorkParkRoute`); null otherwise.
    #[cfg(not(target_arch = "wasm32"))]
    park_route: AtomicPtr<crate::async_rt::event_loop::LoopParker>,
}

fn initial_handler_bits(slot: usize) -> u64 {
    if slot == libc::SIGINT as usize {
        HANDLER_DEFAULT_INT
    } else {
        HANDLER_SIG_DFL
    }
}

impl SignalRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            handlers: std::array::from_fn(|slot| AtomicU64::new(initial_handler_bits(slot))),
            wakeup_fd: AtomicI32::new(-1),
            pending: std::array::from_fn(|_| AtomicU64::new(0)),
            #[cfg(not(target_arch = "wasm32"))]
            park_route: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn handler_bits(&self, signum: i32) -> u64 {
        self.handlers[signum as usize].load(Ordering::SeqCst)
    }

    fn handler_bits_for_return(&self, _py: &PyToken<'_>, signum: i32) -> u64 {
        let bits = self.handler_bits(signum);
        if is_callable_handler_bits(bits) {
            inc_ref_bits(_py, bits);
        }
        bits
    }

    fn replace_handler_retaining(
        &self,
        _py: &PyToken<'_>,
        signum: i32,
        new_handler_bits: u64,
    ) -> u64 {
        if is_callable_handler_bits(new_handler_bits) {
            inc_ref_bits(_py, new_handler_bits);
        }
        self.handlers[signum as usize].swap(new_handler_bits, Ordering::SeqCst)
    }

    fn swap_wakeup_fd(&self, new_fd: i32) -> i32 {
        self.wakeup_fd.swap(new_fd, Ordering::SeqCst)
    }

    fn wakeup_fd(&self) -> i32 {
        self.wakeup_fd.load(Ordering::Relaxed)
    }

    /// Record one delivery of `signum`: pending flag, then the tripped
    /// summary, then wake bytes. Whoever a wake byte reaches can therefore
    /// observe the delivery it announces (bpo-30038). Async-signal-safe: only
    /// atomics and `write(2)`.
    fn record_delivery(&self, signum: i32) {
        let Ok(slot) = usize::try_from(signum) else {
            return;
        };
        if slot == 0 || slot >= MAX_SIGNAL {
            return;
        }
        self.pending[slot].store(1, Ordering::SeqCst);
        SIGNALS_TRIPPED.store(true, Ordering::SeqCst);
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        write_wakeup_byte(self.wakeup_fd.load(Ordering::SeqCst), signum as u8);
        self.notify_parked_owner();
    }

    /// Called only inside the shared notifier admission window.
    fn notify_parked_owner(&self) {
        // Queue slots publish with Release. This fence pairs with route
        // publication's fence before its Acquire queue-readiness recheck:
        // publisher and parker cannot both miss the other's publication.
        std::sync::atomic::fence(Ordering::SeqCst);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let route = self.park_route.load(Ordering::SeqCst);
            if !route.is_null() {
                // SAFETY: the route outlives every admitted notifier; route
                // withdrawal quiesces that same counter before returning.
                unsafe { (*route).unpark() };
            }
        }
    }

    fn clear_for_teardown(&self, _py: &PyToken<'_>) -> bool {
        let mut changed = self.wakeup_fd.swap(-1, Ordering::SeqCst) != -1;
        #[cfg(not(target_arch = "wasm32"))]
        {
            changed |= !self
                .park_route
                .swap(std::ptr::null_mut(), Ordering::SeqCst)
                .is_null();
        }
        let handlers: [u64; MAX_SIGNAL] = std::array::from_fn(|idx| {
            let initial = initial_handler_bits(idx);
            let bits = self.handlers[idx].swap(initial, Ordering::SeqCst);
            changed |= bits != initial;
            changed |= self.pending[idx].swap(0, Ordering::SeqCst) != 0;
            bits
        });
        // All handlers and pending delivery state are detached before a handler
        // finalizer can rearm any sibling signal slot.
        for old_bits in handlers {
            if is_callable_handler_bits(old_bits) {
                dec_ref_bits(_py, old_bits);
            }
        }
        changed
    }

    #[cfg(test)]
    fn pending_for_test(&self, signum: i32) -> bool {
        self.pending[signum as usize].load(Ordering::SeqCst) != 0
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn write_wakeup_byte(fd: i32, byte: u8) {
    if fd >= 0 {
        unsafe {
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
        }
    }
}

/// Lifecycle publication is the only writer of the active signal authority.
/// Isolates cannot acquire process-global OS dispositions or wake ownership.
pub(crate) fn signal_runtime_state_publish(
    state: &crate::state::runtime_state::RuntimeState,
) -> bool {
    if !crate::state::runtime_state::owns_process_cpython_state(state) {
        return false;
    }
    let published = ACTIVE_SIGNAL_STATE
        .compare_exchange(
            std::ptr::null_mut(),
            (&state.signal as *const SignalRuntimeState).cast_mut(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok();
    if published {
        UNHANDLED_KEYBOARD_INTERRUPT.store(false, Ordering::SeqCst);
    }
    published
}

#[inline]
fn active_signal_runtime(py: &PyToken<'_>) -> bool {
    std::ptr::eq(
        ACTIVE_SIGNAL_STATE.load(Ordering::SeqCst),
        &runtime_state(py).signal,
    )
}

fn require_active_signal_runtime(py: &PyToken<'_>) -> Result<(), u64> {
    if active_signal_runtime(py) {
        Ok(())
    } else {
        Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            "signals are owned by the main interpreter",
        ))
    }
}

fn signal_runtime_state_deactivate(signal: &SignalRuntimeState) -> bool {
    let ptr = (signal as *const SignalRuntimeState).cast_mut();
    ACTIVE_SIGNAL_STATE
        .compare_exchange(
            ptr,
            std::ptr::null_mut(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn active_signal_state() -> Option<&'static SignalRuntimeState> {
    let ptr = ACTIVE_SIGNAL_STATE.load(Ordering::SeqCst);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

/// Run `record` against the active state inside the delivery admission
/// window. Async-signal-safe.
#[inline]
fn with_admitted_state(record: impl FnOnce(&'static SignalRuntimeState)) {
    ASYNC_WORK_NOTIFIERS_IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    if let Some(signal) = active_signal_state() {
        record(signal);
    }
    ASYNC_WORK_NOTIFIERS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}

/// Notify after the C pending-call ring has published a slot. No queue lock,
/// runtime lock, GIL, TLS lookup, allocation, or Python wakeup-fd write is used.
pub(crate) fn notify_pending_calls() {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let saved = raw_errno::get();
    with_admitted_state(SignalRuntimeState::notify_parked_owner);
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    raw_errno::set(saved);
}

/// Wait until every notifier admitted before the caller's withdrawal has left.
/// Admitted bodies never block, so the wait is bounded by their scheduling.
/// The GIL is released first when this thread holds it through ordinary TLS
/// custody, so Python threads never wait on another thread's preempted handler.
fn quiesce_admitted_notifications() {
    if ASYNC_WORK_NOTIFIERS_IN_FLIGHT.load(Ordering::SeqCst) == 0 {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _release = crate::GIL_DEPTH
        .try_with(|depth| depth.get() > 0)
        .unwrap_or(false)
        .then(crate::GilReleaseGuard::suspend);
    let mut spins = 0u32;
    while ASYNC_WORK_NOTIFIERS_IN_FLIGHT.load(Ordering::SeqCst) != 0 {
        if spins < 64 {
            std::hint::spin_loop();
            spins += 1;
        } else {
            std::thread::yield_now();
        }
    }
}

pub(crate) fn signal_clear_state(
    _py: &PyToken<'_>,
    state: &crate::state::runtime_state::RuntimeState,
) -> bool {
    let deactivated = signal_runtime_state_deactivate(&state.signal);
    if deactivated {
        // A fresh inactive isolate still has the SIGINT default-int sentinel.
        // Only the runtime that owned the active route may reset OS handlers.
        reset_os_handlers_for_teardown(&state.signal);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let route_published = !state
        .signal
        .park_route
        .swap(std::ptr::null_mut(), Ordering::SeqCst)
        .is_null();
    #[cfg(target_arch = "wasm32")]
    let route_published = false;
    // No delivery admitted before the withdrawals above may touch this state
    // once teardown releases it.
    quiesce_admitted_notifications();
    // The summary describes the active state only; an isolate's teardown must
    // not drop the primary runtime's undispatched deliveries.
    let tripped = deactivated && SIGNALS_TRIPPED.swap(false, Ordering::SeqCst);
    state.signal.clear_for_teardown(_py) | deactivated | tripped | route_published
}

/// A retained callable (not a sentinel disposition).
fn is_callable_handler_bits(bits: u64) -> bool {
    bits != HANDLER_SIG_DFL && bits != HANDLER_SIG_IGN && bits != HANDLER_DEFAULT_INT
}

/// Handled by Python: a callable or SIGINT's `default_int_handler` disposition.
fn is_python_handler_bits(bits: u64) -> bool {
    bits != HANDLER_SIG_DFL && bits != HANDLER_SIG_IGN
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn install_os_handler(signum: i32, handler_bits: u64) -> Result<(), std::io::Error> {
    let _guard = SIGACTION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let result = unsafe {
        match handler_bits {
            // SIGINT's `default_int_handler` disposition keeps the OS default.
            HANDLER_SIG_DFL | HANDLER_DEFAULT_INT => libc::signal(signum, libc::SIG_DFL),
            HANDLER_SIG_IGN => libc::signal(signum, libc::SIG_IGN),
            _ => libc::signal(
                signum,
                molt_c_signal_handler as *const () as libc::sighandler_t,
            ),
        }
    };
    if result == libc::SIG_ERR {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(any(not(unix), target_arch = "wasm32"))]
fn install_os_handler(_signum: i32, _handler_bits: u64) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn reset_os_handlers_for_teardown(signal: &SignalRuntimeState) {
    let _guard = SIGACTION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let nsig = effective_nsig().min(MAX_SIGNAL as i64);
    for signum in 1..nsig {
        if signal.handler_bits(signum as i32) != HANDLER_SIG_DFL {
            unsafe {
                libc::signal(signum as libc::c_int, libc::SIG_DFL);
            }
        }
    }
}

#[cfg(any(not(unix), target_arch = "wasm32"))]
fn reset_os_handlers_for_teardown(_signal: &SignalRuntimeState) {}

// ── errno across asynchronous handlers ────────────────────────────────────

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod raw_errno {
    #[cfg(any(target_os = "dragonfly", target_os = "vxworks", target_os = "rtems"))]
    compile_error!("the raw signal handler must preserve errno; add this target's errno accessor");

    unsafe extern "C" {
        #[cfg_attr(
            any(
                target_os = "linux",
                target_os = "emscripten",
                target_os = "fuchsia",
                target_os = "l4re",
                target_os = "hurd"
            ),
            link_name = "__errno_location"
        )]
        #[cfg_attr(
            any(
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "cygwin",
                target_os = "android",
                target_os = "redox"
            ),
            link_name = "__errno"
        )]
        #[cfg_attr(
            any(target_os = "solaris", target_os = "illumos"),
            link_name = "___errno"
        )]
        #[cfg_attr(target_os = "nto", link_name = "__get_errno_ptr")]
        #[cfg_attr(
            any(target_os = "freebsd", target_vendor = "apple"),
            link_name = "__error"
        )]
        #[cfg_attr(target_os = "haiku", link_name = "_errnop")]
        #[cfg_attr(target_os = "aix", link_name = "_Errno")]
        fn errno_location() -> *mut libc::c_int;
    }

    #[inline]
    pub(super) fn get() -> libc::c_int {
        unsafe { *errno_location() }
    }

    #[inline]
    pub(super) fn set(value: libc::c_int) {
        unsafe { *errno_location() = value }
    }
}

// ── Raw C signal handler ──────────────────────────────────────────────────
//
// Installed only for Python-handled signals. It records the delivery and
// writes wake bytes; Python callables run later on the registered main thread
// at a safepoint, matching CPython's deferred signal architecture.

#[cfg(all(unix, not(target_arch = "wasm32")))]
extern "C" fn molt_c_signal_handler(signum: libc::c_int) {
    // CPython `signal_handler`: an asynchronous handler must not change errno
    // under the code it interrupted (bpo-10311).
    let saved = raw_errno::get();
    with_admitted_state(|signal| signal.record_delivery(signum));
    raw_errno::set(saved);
}

// ── Main-thread dispatch ──────────────────────────────────────────────────

/// The thread that runs Python signal handlers: the one registered as process
/// main by the winning runtime initialization (CPython's
/// `_Py_ThreadCanHandleSignals`). The pending-call authority owns the identity.
#[inline]
pub(crate) fn signal_owner_thread() -> bool {
    molt_cpython_abi::api::pending_calls::current_thread_is_main()
}

/// The process-main thread attached to the active process runtime. Thread
/// identity alone is insufficient when that thread enters an isolate.
#[inline]
pub(crate) fn async_work_owner(py: &PyToken<'_>) -> bool {
    signal_owner_thread() && active_signal_runtime(py)
}

/// The existing signal summary and pending-call ring remain the work facts.
#[inline]
pub(crate) fn async_work_pending() -> bool {
    signal_tripped() || molt_cpython_abi::api::pending_calls::has_pending_calls()
}

/// A delivery is recorded and not yet dispatched.
#[inline]
pub(crate) fn signal_tripped() -> bool {
    SIGNALS_TRIPPED.load(Ordering::SeqCst)
}

/// Eval-breaker entry, called first by `service_async_work`: one relaxed load
/// unless a delivery is recorded. Returns true when a handler raised; its
/// exception is pending.
#[inline]
pub(crate) fn signal_safepoint(py: &PyToken<'_>) -> bool {
    if !SIGNALS_TRIPPED.load(Ordering::Relaxed) {
        return false;
    }
    signal_safepoint_slow(py)
}

#[cold]
#[inline(never)]
fn signal_safepoint_slow(py: &PyToken<'_>) -> bool {
    // Handlers run at instruction boundaries, never under an exception that is
    // already propagating; the next clean safepoint dispatches them.
    if !async_work_owner(py) || exception_pending(py) {
        return false;
    }
    run_pending_handlers(py)
}

/// CPython `_PyErr_CheckSignalsTstate`: run the Python handlers of recorded
/// deliveries in signal-number order with `(signum, None)`. Callers must be the
/// signal owner. Returns true when a handler raised; deliveries after it stay
/// recorded for the next safepoint.
fn run_pending_handlers(py: &PyToken<'_>) -> bool {
    if !SIGNALS_TRIPPED.swap(false, Ordering::SeqCst) {
        return false;
    }
    let signal = &runtime_state(py).signal;
    for slot in 1..MAX_SIGNAL {
        if signal.pending[slot].swap(0, Ordering::SeqCst) == 0 {
            continue;
        }
        let signum = slot as i32;
        let signum_bits = int_bits_from_i64(py, signum as i64);
        let raised = match signal.handler_bits_for_return(py, signum) {
            HANDLER_SIG_DFL | HANDLER_SIG_IGN => {
                // bpo-43406: the handler changed after the delivery was recorded.
                report_handler_race(py, signum);
                false
            }
            HANDLER_DEFAULT_INT => {
                let _ = molt_signal_default_int_handler(signum_bits, MoltObject::none().bits());
                exception_pending(py)
            }
            handler => {
                let result = unsafe {
                    crate::call::dispatch::call_callable2(
                        py,
                        handler,
                        signum_bits,
                        MoltObject::none().bits(),
                    )
                };
                dec_ref_bits(py, handler);
                let raised = exception_pending(py);
                if !raised && !obj_from_bits(result).is_none() {
                    dec_ref_bits(py, result);
                }
                raised
            }
        };
        if raised {
            // Later deliveries stay recorded for the next safepoint. CPython
            // re-arms unconditionally; re-arming only when one remains avoids a
            // spurious wake of a parked loop, and a delivery recorded meanwhile
            // sets the summary itself.
            if signal.pending[slot + 1..]
                .iter()
                .any(|pending| pending.load(Ordering::SeqCst) != 0)
            {
                SIGNALS_TRIPPED.store(true, Ordering::SeqCst);
            }
            return true;
        }
    }
    false
}

fn report_handler_race(py: &PyToken<'_>, signum: i32) {
    let message = format!("Signal {signum} ignored due to race condition");
    crate::builtins::exceptions::run_unraisable(py, MoltObject::none().bits(), None, || {
        let _ = raise_exception::<u64>(py, "OSError", &message);
    });
}

/// CPython `PyErr_CheckSignals`: run pending handlers when called on the signal
/// owner. Returns -1 with the handler's exception pending in the runtime.
pub(crate) fn signal_check_signals(py: &PyToken<'_>) -> i32 {
    if signal_safepoint(py) { -1 } else { 0 }
}

/// CPython `PyErr_SetInterruptEx`: simulate a delivery of `signum` without an
/// OS signal. Async-signal-safe and callable from any thread without the GIL.
/// Returns -1 for an out-of-range number; a signal not handled by Python
/// (SIG_DFL or SIG_IGN) is ignored.
pub(crate) fn signal_set_interrupt(signum: i64) -> i32 {
    if signum < 1 || signum >= effective_nsig() {
        return -1;
    }
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let saved = raw_errno::get();
    with_admitted_state(|signal| {
        if is_python_handler_bits(signal.handler_bits(signum as i32)) {
            signal.record_delivery(signum as i32);
        }
    });
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    raw_errno::set(saved);
    0
}

/// CPython `PyOS_InterruptOccurred`: on the signal owner, consume a recorded
/// SIGINT delivery without running its handler.
pub(crate) fn signal_interrupt_occurred(py: &PyToken<'_>) -> bool {
    if !async_work_owner(py) {
        return false;
    }
    runtime_state(py).signal.pending[libc::SIGINT as usize].swap(0, Ordering::SeqCst) != 0
}

/// CPython `_PyRuntime.signals.unhandled_keyboard_interrupt`: an exception of
/// exactly type KeyboardInterrupt escaped the application.
static UNHANDLED_KEYBOARD_INTERRUPT: AtomicBool = AtomicBool::new(false);

/// Called by the uncaught-exception reporter for an exact KeyboardInterrupt.
pub(crate) fn signal_note_unhandled_keyboard_interrupt(py: &PyToken<'_>) {
    if active_signal_runtime(py) {
        UNHANDLED_KEYBOARD_INTERRUPT.store(true, Ordering::SeqCst);
    }
}

/// CPython `exit_sigint` (bpo-1054041), applied by the process-exit boundary
/// after finalization so a calling shell observes the interrupt: on Unix it
/// re-delivers SIGINT under SIG_DFL and returns only if the process survives.
pub(crate) fn signal_exit_status_after_finalization(code: i32) -> i32 {
    if !UNHANDLED_KEYBOARD_INTERRUPT.load(Ordering::SeqCst) {
        return code;
    }
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let _guard = SIGACTION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
                eprintln!("signal: {}", std::io::Error::last_os_error());
            } else {
                libc::kill(libc::getpid(), libc::SIGINT);
            }
        }
    }
    if cfg!(windows) {
        // STATUS_CONTROL_C_EXIT: cmd.exe reports ^C and offers to terminate.
        0xC000_013A_u32 as i32
    } else {
        libc::SIGINT + 128
    }
}

/// Routes deliveries to the parker of the loop the registered main thread is
/// about to block on, for exactly one park.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct AsyncWorkParkRoute {
    signal: &'static SignalRuntimeState,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncWorkParkRoute {
    /// Publish `parker` as the delivery wake route. Returns `None`, with
    /// nothing left published, when a delivery is already recorded: the caller
    /// must return to its safepoint instead of blocking. Otherwise every later
    /// delivery either precedes the re-check below and is seen by it, or loads
    /// the published route and posts the parker's token.
    pub(crate) fn publish(
        py: &PyToken<'_>,
        parker: &crate::async_rt::event_loop::LoopParker,
    ) -> Option<Self> {
        if !async_work_owner(py) || async_work_pending() {
            return None;
        }
        let signal = &runtime_state(py).signal;
        signal.park_route.store(
            (parker as *const crate::async_rt::event_loop::LoopParker).cast_mut(),
            Ordering::SeqCst,
        );
        let route = Self { signal };
        // Pairs with the publisher's queue/summary publication, fence, then
        // route load. This also closes the C pending-call arm/recheck race.
        std::sync::atomic::fence(Ordering::SeqCst);
        if async_work_pending() {
            return None;
        }
        Some(route)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AsyncWorkParkRoute {
    fn drop(&mut self) {
        self.signal
            .park_route
            .store(std::ptr::null_mut(), Ordering::SeqCst);
        // The parker may be released as soon as the park returns.
        quiesce_admitted_notifications();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn async_work_route_published_for_test() -> bool {
    let mut published = false;
    with_admitted_state(|signal| {
        published = !signal.park_route.load(Ordering::SeqCst).is_null();
    });
    published
}

#[cfg(test)]
pub(crate) fn signal_record_delivery_for_test(py: &PyToken<'_>, signum: i32) {
    runtime_state(py).signal.record_delivery(signum);
}

#[cfg(all(test, unix, not(target_arch = "wasm32")))]
pub(crate) fn deliver_raw_signal_for_test(signum: i32) {
    molt_c_signal_handler(signum);
}

// ── Internal helpers ──────────────────────────────────────────────────────

#[inline]
fn effective_nsig() -> i64 {
    #[cfg(target_os = "macos")]
    {
        32_i64.min(MAX_SIGNAL as i64)
    }
    #[cfg(target_os = "ios")]
    {
        32_i64.min(MAX_SIGNAL as i64)
    }
    #[cfg(all(
        unix,
        not(target_arch = "wasm32"),
        not(any(target_os = "macos", target_os = "ios"))
    ))]
    {
        65_i64.min(MAX_SIGNAL as i64)
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        MAX_SIGNAL as i64
    }
}

fn sig_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<i32, u64> {
    let obj = obj_from_bits(bits);
    let nsig = effective_nsig();
    match to_i64(obj) {
        Some(v) if v > 0 && v < nsig => Ok(v as i32),
        Some(v) if v <= 0 => Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "signal number must be positive",
        )),
        Some(_) => Err(raise_exception::<u64>(
            _py,
            "ValueError",
            &format!("signal number out of range (max {})", nsig - 1),
        )),
        None => Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "signal number must be int",
        )),
    }
}

/// The cached `molt_signal_default_int_handler` intrinsic: the one object
/// `signal.default_int_handler` and `_signal.default_int_handler` both name.
fn default_int_handler_object(py: &PyToken<'_>) -> Result<u64, u64> {
    match crate::intrinsics::registry::try_resolve_intrinsic_func(
        py,
        "molt_signal_default_int_handler",
        true,
    ) {
        Ok(Some(bits)) => Ok(bits),
        Ok(None) => Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            "signal.default_int_handler is unavailable",
        )),
        Err(()) => Err(MoltObject::none().bits()),
    }
}

/// Classify a Python handler argument into handler-table bits.
fn handler_bits_from_py(py: &PyToken<'_>, signum: i32, handler_bits: u64) -> Result<u64, u64> {
    let handler = obj_from_bits(handler_bits);
    if let Some(value) = to_i64(handler) {
        return match value {
            SIG_DFL_INT => Ok(HANDLER_SIG_DFL),
            SIG_IGN_INT => Ok(HANDLER_SIG_IGN),
            _ => Err(raise_exception::<u64>(py, "TypeError", HANDLER_TYPE_ERROR)),
        };
    }
    if handler.is_none() || !crate::builtins::callable::is_callable_impl(py, handler_bits) {
        return Err(raise_exception::<u64>(py, "TypeError", HANDLER_TYPE_ERROR));
    }
    if signum == libc::SIGINT {
        let default_int = default_int_handler_object(py)?;
        let is_default = default_int == handler_bits;
        dec_ref_bits(py, default_int);
        if is_default {
            return Ok(HANDLER_DEFAULT_INT);
        }
    }
    Ok(handler_bits)
}

/// Project owned handler-table bits to the Python-visible handler.
fn handler_bits_into_py(py: &PyToken<'_>, bits: u64) -> u64 {
    match bits {
        HANDLER_SIG_DFL => int_bits_from_i64(py, SIG_DFL_INT),
        HANDLER_SIG_IGN => int_bits_from_i64(py, SIG_IGN_INT),
        HANDLER_DEFAULT_INT => default_int_handler_object(py).unwrap_or_else(|err| err),
        // A retained callable: the caller's reference transfers.
        callable => callable,
    }
}

// ── Public intrinsics ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_signal(signum_bits: u64, handler_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalSignal, AuditArgs::None)
        {
            return err;
        }
        if !async_work_owner(_py) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "signal only works in main thread of the main interpreter",
            );
        }
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let new_handler_bits = match handler_bits_from_py(_py, signum, handler_bits) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        // CPython checks pending signals before changing a handler, so a
        // delivery recorded under the old handler is dispatched to it.
        if run_pending_handlers(_py) {
            return MoltObject::none().bits();
        }
        if let Err(err) = install_os_handler(signum, new_handler_bits) {
            return raise_exception::<u64>(_py, "OSError", &err.to_string());
        }
        let state = runtime_state(_py);
        let old_bits = state
            .signal
            .replace_handler_retaining(_py, signum, new_handler_bits);
        handler_bits_into_py(_py, old_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_getsignal(signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalGetsignal, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let bits = runtime_state(_py)
            .signal
            .handler_bits_for_return(_py, signum);
        handler_bits_into_py(_py, bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_raise_signal(signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalRaise, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let handler = runtime_state(_py).signal.handler_bits(signum);
        if handler == HANDLER_DEFAULT_INT
            || (!OS_SIGNAL_DELIVERY && is_python_handler_bits(handler))
        {
            // A Python-visible handler with no OS handler behind it: simulate
            // the delivery, as `PyErr_SetInterruptEx` does.
            let _ = signal_set_interrupt(signum as i64);
        } else {
            #[cfg(all(unix, not(target_arch = "wasm32")))]
            {
                let rc = unsafe { libc::raise(signum) };
                if rc != 0 {
                    return raise_exception::<u64>(
                        _py,
                        "OSError",
                        &std::io::Error::last_os_error().to_string(),
                    );
                }
            }
            #[cfg(any(not(unix), target_arch = "wasm32"))]
            {
                // No OS signal delivery on this host: SIG_IGN has no effect,
                // and the OS default action of SIG_DFL cannot be performed.
                if handler == HANDLER_SIG_DFL {
                    return raise_exception::<u64>(
                        _py,
                        "OSError",
                        &format!(
                            "the default action of signal {signum} is not supported on this platform"
                        ),
                    );
                }
            }
        }
        // CPython handles the raised signal before `raise_signal` returns.
        if async_work_owner(_py) && run_pending_handlers(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

/// `_thread.interrupt_main(signum)`: CPython `PyErr_SetInterruptEx`. Simulates a
/// delivery for the main thread's next safepoint; no OS signal is raised.
#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_set_interrupt(signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalRaise, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        let Some(signum) = to_i64(obj_from_bits(signum_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "signal number must be int");
        };
        if signal_set_interrupt(signum) != 0 {
            return raise_exception::<u64>(_py, "ValueError", "signal number out of range");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_alarm(seconds_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalAlarm, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let secs = to_i64(obj_from_bits(seconds_bits)).unwrap_or(0).max(0) as u32;
            let prev = unsafe { libc::alarm(secs) };
            int_bits_from_i64(_py, prev as i64)
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = seconds_bits;
            raise_exception::<u64>(
                _py,
                "OSError",
                "signal.alarm not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_pause() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalPause, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            {
                let _release = crate::GilReleaseGuard::suspend();
                unsafe { libc::pause() };
            }
            // CPython: `pause` returns after a handler ran on this thread; run
            // the Python handlers before returning.
            if async_work_owner(_py) && run_pending_handlers(_py) {
                return MoltObject::none().bits();
            }
            MoltObject::none().bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            raise_exception::<u64>(
                _py,
                "OSError",
                "signal.pause not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_set_wakeup_fd(fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalSetWakeupFd, AuditArgs::None)
        {
            return err;
        }
        if !async_work_owner(_py) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "set_wakeup_fd only works in main thread of the main interpreter",
            );
        }
        let Some(raw_fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "fd must be an integer");
        };
        let Ok(new_fd) = i32::try_from(raw_fd) else {
            return raise_exception::<u64>(_py, "ValueError", "fd is out of range");
        };
        if new_fd < -1 {
            return raise_exception::<u64>(_py, "ValueError", "invalid fd");
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        if new_fd >= 0 {
            let flags = unsafe { libc::fcntl(new_fd, libc::F_GETFL) };
            if flags < 0 {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            if flags & libc::O_NONBLOCK == 0 {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "the fd must be in non-blocking mode",
                );
            }
        }
        let state = runtime_state(_py);
        let old_fd = state.signal.swap_wakeup_fd(new_fd);
        // After this returns no delivery can still write the replaced fd, so
        // the caller may close it.
        quiesce_admitted_notifications();
        int_bits_from_i64(_py, old_fd as i64)
    })
}

// ── Signal number constants ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigabrt() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGABRT as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigfpe() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGFPE as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigill() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGILL as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigint() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGINT as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigsegv() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGSEGV as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigterm() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTERM as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sighup() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGHUP as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 1_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigquit() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGQUIT as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 3_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigusr1() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGUSR1 as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 10_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigusr2() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGUSR2 as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 12_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigchld() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGCHLD as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 17_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigalrm() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGALRM as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 14_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigpipe() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGPIPE as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 13_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_dfl() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, SIG_DFL_INT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_ign() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, SIG_IGN_INT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_nsig() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, effective_nsig()) })
}

// ── Extended signal number constants ────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_block() -> u64 {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIG_BLOCK as i64) })
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 0_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_unblock() -> u64 {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIG_UNBLOCK as i64) })
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 1_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_setmask() -> u64 {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIG_SETMASK as i64) })
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 2_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigbus() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGBUS as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 7_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigcont() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGCONT as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 18_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigstop() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGSTOP as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 19_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigtstp() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTSTP as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 20_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigttin() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTTIN as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 21_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigttou() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTTOU as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 22_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigxcpu() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGXCPU as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 24_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigxfsz() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGXFSZ as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 25_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigvtalrm() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGVTALRM as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 26_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigprof() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGPROF as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 27_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigwinch() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGWINCH as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 28_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigsys() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGSYS as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 31_i64) })
    }
}

// ── POSIX signal functions ─────────────────────────────────────────────────

/// Convert a list of signal ints (u64 bits) into a `sigset_t`.
#[cfg(all(unix, not(target_arch = "wasm32")))]
unsafe fn bits_to_sigset(_py: &PyToken<'_>, list_ptr: *mut u8) -> Result<libc::sigset_t, u64> {
    unsafe {
        let nsig = effective_nsig();
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        let len = crate::builtins::containers::list_len(list_ptr);
        for i in 0..len {
            let mut elem_bits = 0;
            let _ = crate::object::seq_access::read_item_gil_borrowed(list_ptr, i, &mut elem_bits);
            let elem_obj = obj_from_bits(elem_bits);
            match to_i64(elem_obj) {
                Some(v) if v > 0 && v < nsig => {
                    libc::sigaddset(&mut set, v as libc::c_int);
                }
                _ => {
                    return Err(raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "invalid signal number in set",
                    ));
                }
            }
        }
        Ok(set)
    }
}

/// Convert a `sigset_t` back to a list of signal number bits.
#[cfg(all(unix, not(target_arch = "wasm32")))]
unsafe fn sigset_to_list_bits(_py: &PyToken<'_>, set: &libc::sigset_t) -> u64 {
    unsafe {
        let nsig = effective_nsig() as libc::c_int;
        let mut elems = Vec::new();
        for sig in 1..nsig {
            if libc::sigismember(set, sig) == 1 {
                elems.push(int_bits_from_i64(_py, sig as i64));
            }
        }
        let list_ptr = alloc_list(_py, &elems);
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_default_int_handler(_signum_bits: u64, _frame_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = (_signum_bits, _frame_bits);
        raise_exception::<u64>(_py, "KeyboardInterrupt", "")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_strsignal(signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let cstr = unsafe { libc::strsignal(signum) };
            if cstr.is_null() {
                return MoltObject::none().bits();
            }
            let s = unsafe { std::ffi::CStr::from_ptr(cstr) };
            let bytes = s.to_bytes();
            let ptr = alloc_string(_py, bytes);
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            // Static lookup table for common signal descriptions on WASM.
            let desc: Option<&[u8]> = match signum {
                1 => Some(b"Hangup"),
                2 => Some(b"Interrupt"),
                3 => Some(b"Quit"),
                4 => Some(b"Illegal instruction"),
                5 => Some(b"Trace/BPT trap"),
                6 => Some(b"Aborted"),
                7 => Some(b"Bus error"),
                8 => Some(b"Floating point exception"),
                9 => Some(b"Killed"),
                10 => Some(b"User defined signal 1"),
                11 => Some(b"Segmentation fault"),
                12 => Some(b"User defined signal 2"),
                13 => Some(b"Broken pipe"),
                14 => Some(b"Alarm clock"),
                15 => Some(b"Terminated"),
                17 => Some(b"Child exited"),
                18 => Some(b"Continued"),
                19 => Some(b"Stopped (signal)"),
                20 => Some(b"Stopped"),
                21 => Some(b"Stopped (tty input)"),
                22 => Some(b"Stopped (tty output)"),
                24 => Some(b"CPU time limit exceeded"),
                25 => Some(b"File size limit exceeded"),
                26 => Some(b"Virtual timer expired"),
                27 => Some(b"Profiling timer expired"),
                28 => Some(b"Window changed"),
                31 => Some(b"Bad system call"),
                _ => None,
            };
            match desc {
                Some(bytes) => {
                    let ptr = alloc_string(_py, bytes);
                    MoltObject::from_ptr(ptr).bits()
                }
                None => MoltObject::none().bits(),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_pthread_sigmask(how_bits: u64, mask_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = crate::require_operation(
            _py,
            crate::OperationId::SignalPthreadSigmask,
            AuditArgs::None,
        ) {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let how_obj = obj_from_bits(how_bits);
            let how = match to_i64(how_obj) {
                Some(v) => v as libc::c_int,
                None => {
                    return raise_exception::<u64>(_py, "TypeError", "how must be an integer");
                }
            };
            // Validate how value against platform constants.
            if how != libc::SIG_BLOCK && how != libc::SIG_UNBLOCK && how != libc::SIG_SETMASK {
                return raise_exception::<u64>(_py, "ValueError", "invalid value for how");
            }

            let mask_obj = obj_from_bits(mask_bits);
            let mask_ptr = match mask_obj.as_ptr() {
                Some(p) => p,
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "mask must be a list of signal numbers",
                    );
                }
            };
            let new_set = match unsafe { bits_to_sigset(_py, mask_ptr) } {
                Ok(s) => s,
                Err(e) => return e,
            };
            let mut old_set: libc::sigset_t = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::pthread_sigmask(how, &new_set, &mut old_set) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            unsafe { sigset_to_list_bits(_py, &old_set) }
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = (how_bits, mask_bits);
            raise_exception::<u64>(
                _py,
                "OSError",
                "pthread_sigmask not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_pthread_kill(thread_id_bits: u64, signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalPthreadKill, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let tid_obj = obj_from_bits(thread_id_bits);
            let tid = match to_i64(tid_obj) {
                Some(v) => v as libc::pthread_t,
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "thread_id must be an integer",
                    );
                }
            };
            let signum = match sig_from_bits(_py, signum_bits) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let rc = unsafe { libc::pthread_kill(tid, signum) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::from_raw_os_error(rc).to_string(),
                );
            }
            MoltObject::none().bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = (thread_id_bits, signum_bits);
            raise_exception::<u64>(
                _py,
                "OSError",
                "pthread_kill not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigpending() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalSigpending, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::sigpending(&mut set) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            unsafe { sigset_to_list_bits(_py, &set) }
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            raise_exception::<u64>(_py, "OSError", "sigpending not available on this platform")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigwait(sigset_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::SignalSigwait, AuditArgs::None)
        {
            return err;
        }
        if let Err(err) = require_active_signal_runtime(_py) {
            return err;
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let sigset_obj = obj_from_bits(sigset_bits);
            let sigset_ptr = match sigset_obj.as_ptr() {
                Some(p) => p,
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "sigset must be a list of signal numbers",
                    );
                }
            };
            let wait_set = match unsafe { bits_to_sigset(_py, sigset_ptr) } {
                Ok(s) => s,
                Err(e) => return e,
            };
            let mut sig: libc::c_int = 0;
            // CPython releases the GIL for the whole wait.
            let rc = {
                let _release = crate::GilReleaseGuard::suspend();
                unsafe { libc::sigwait(&wait_set, &mut sig) }
            };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::from_raw_os_error(rc).to_string(),
                );
            }
            int_bits_from_i64(_py, sig as i64)
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = sigset_bits;
            raise_exception::<u64>(_py, "OSError", "sigwait not available on this platform")
        }
    })
}

// ── Valid signals set ──────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_valid_signals() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Enumerate known valid signal numbers for this platform.
        let valid: Vec<i64> = {
            #[cfg(unix)]
            {
                let candidates: &[i64] = &[
                    libc::SIGABRT as i64,
                    libc::SIGFPE as i64,
                    libc::SIGHUP as i64,
                    libc::SIGILL as i64,
                    libc::SIGINT as i64,
                    libc::SIGPIPE as i64,
                    libc::SIGQUIT as i64,
                    libc::SIGSEGV as i64,
                    libc::SIGTERM as i64,
                    libc::SIGUSR1 as i64,
                    libc::SIGUSR2 as i64,
                    libc::SIGCHLD as i64,
                    libc::SIGALRM as i64,
                    libc::SIGBUS as i64,
                    libc::SIGTRAP as i64,
                    libc::SIGTSTP as i64,
                    libc::SIGCONT as i64,
                    libc::SIGWINCH as i64,
                    libc::SIGSTOP as i64,
                    libc::SIGTTIN as i64,
                    libc::SIGTTOU as i64,
                    libc::SIGXCPU as i64,
                    libc::SIGXFSZ as i64,
                    libc::SIGVTALRM as i64,
                    libc::SIGPROF as i64,
                    libc::SIGSYS as i64,
                ];
                candidates.to_vec()
            }
            #[cfg(not(unix))]
            {
                vec![
                    libc::SIGABRT as i64,
                    libc::SIGFPE as i64,
                    libc::SIGILL as i64,
                    libc::SIGINT as i64,
                    libc::SIGSEGV as i64,
                    libc::SIGTERM as i64,
                ]
            }
        };
        let int_bits: Vec<u64> = valid.iter().map(|&v| int_bits_from_i64(_py, v)).collect();
        let set_ptr = alloc_set_with_entries(_py, &int_bits);
        if set_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(set_ptr).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        HANDLER_DEFAULT_INT, HANDLER_SIG_DFL, HANDLER_SIG_IGN, SignalRuntimeState,
        is_callable_handler_bits, is_python_handler_bits,
    };
    use crate::{MoltObject, alloc_string, dec_ref_bits};
    use std::sync::atomic::Ordering as AtomicOrdering;

    unsafe fn ref_count(ptr: *mut u8) -> u32 {
        unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn signal_runtime_state_retains_getsignal_and_clears_handlers() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = SignalRuntimeState::new();
            let ptr = alloc_string(_py, b"signal handler sentinel");
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            assert_eq!(unsafe { ref_count(ptr) }, 1);

            let old = state.replace_handler_retaining(_py, 2, bits);
            assert_eq!(old, HANDLER_DEFAULT_INT);
            assert_eq!(unsafe { ref_count(ptr) }, 2);

            let returned = state.handler_bits_for_return(_py, 2);
            assert_eq!(returned, bits);
            assert_eq!(unsafe { ref_count(ptr) }, 3);
            dec_ref_bits(_py, returned);
            assert_eq!(unsafe { ref_count(ptr) }, 2);

            state.clear_for_teardown(_py);
            assert_eq!(unsafe { ref_count(ptr) }, 1);
            dec_ref_bits(_py, bits);
        });
    }

    #[test]
    fn signal_runtime_state_replacement_transfers_old_handler_ownership() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = SignalRuntimeState::new();
            let ptr = alloc_string(_py, b"replace handler sentinel");
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();

            let old = state.replace_handler_retaining(_py, 2, bits);
            assert_eq!(old, HANDLER_DEFAULT_INT);
            assert_eq!(unsafe { ref_count(ptr) }, 2);

            let replaced = state.replace_handler_retaining(_py, 2, HANDLER_SIG_IGN);
            assert_eq!(replaced, bits);
            assert_eq!(unsafe { ref_count(ptr) }, 2);
            dec_ref_bits(_py, replaced);
            assert_eq!(unsafe { ref_count(ptr) }, 1);

            state.clear_for_teardown(_py);
            dec_ref_bits(_py, bits);
        });
    }

    #[test]
    fn signal_runtime_state_clear_resets_wakeup_pending_and_startup_dispositions() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = SignalRuntimeState::new();
            assert_eq!(state.swap_wakeup_fd(9), -1);
            state.pending[2].store(1, AtomicOrdering::SeqCst);
            assert!(state.pending_for_test(2));
            let _ = state.replace_handler_retaining(_py, 2, HANDLER_SIG_IGN);

            assert!(state.clear_for_teardown(_py));

            assert_eq!(state.wakeup_fd(), -1);
            assert!(!state.pending_for_test(2));
            assert_eq!(state.handler_bits(2), HANDLER_DEFAULT_INT);
            // Idempotent: a second pass finds nothing to retire.
            assert!(!state.clear_for_teardown(_py));
        });
    }

    #[test]
    fn signal_handler_sentinels_are_not_callable_handlers() {
        assert!(!is_callable_handler_bits(HANDLER_SIG_DFL));
        assert!(!is_callable_handler_bits(HANDLER_SIG_IGN));
        assert!(!is_callable_handler_bits(HANDLER_DEFAULT_INT));
        assert!(is_callable_handler_bits(0x1000));
        assert!(is_python_handler_bits(HANDLER_DEFAULT_INT));
        assert!(!is_python_handler_bits(HANDLER_SIG_DFL));
        assert!(!is_python_handler_bits(HANDLER_SIG_IGN));
    }

    #[test]
    fn startup_sigint_is_the_default_int_disposition_and_other_slots_are_default() {
        let state = SignalRuntimeState::new();
        assert_eq!(state.handler_bits(libc::SIGINT), HANDLER_DEFAULT_INT);
        assert_eq!(state.handler_bits(libc::SIGTERM), HANDLER_SIG_DFL);
    }
}

/// Delivery, dispatch and retirement protocol tests against the runtime's active
/// signal state. Each test leaves no recorded delivery behind.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod delivery_tests {
    use super::*;
    use std::sync::atomic::AtomicI64;
    use std::time::Duration;

    static RECORDED_SIGNUM: AtomicI64 = AtomicI64::new(0);
    static RECORDED_FRAME_NONE: AtomicBool = AtomicBool::new(false);

    extern "C" fn record_delivery_handler(signum_bits: u64, frame_bits: u64) -> u64 {
        RECORDED_SIGNUM.store(
            to_i64(obj_from_bits(signum_bits)).unwrap_or(-1),
            Ordering::SeqCst,
        );
        RECORDED_FRAME_NONE.store(obj_from_bits(frame_bits).is_none(), Ordering::SeqCst);
        MoltObject::none().bits()
    }

    extern "C" fn raising_handler(_signum_bits: u64, _frame_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, { raise_exception::<u64>(py, "ValueError", "handler") })
    }

    fn handler_object(py: &PyToken<'_>, address: *const ()) -> u64 {
        let ptr = crate::object::builders::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            2,
        );
        assert!(!ptr.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(ptr, address) };
        MoltObject::from_ptr(ptr).bits()
    }

    /// Install `bits` in the active state (no OS handler) and return the old
    /// value for `restore`.
    fn install(py: &PyToken<'_>, signum: i32, bits: u64) -> u64 {
        runtime_state(py)
            .signal
            .replace_handler_retaining(py, signum, bits)
    }

    fn restore(py: &PyToken<'_>, signum: i32, old: u64) {
        let current =
            runtime_state(py).signal.handlers[signum as usize].swap(old, Ordering::SeqCst);
        if is_callable_handler_bits(current) {
            dec_ref_bits(py, current);
        }
    }

    fn take_pending_exception_kind(py: &PyToken<'_>) -> String {
        assert!(exception_pending(py), "expected a pending exception");
        let raised = crate::molt_exception_last();
        let kind = crate::type_name(py, obj_from_bits(raised)).to_string();
        crate::molt_exception_clear();
        dec_ref_bits(py, raised);
        kind
    }

    #[cfg(unix)]
    #[test]
    fn inactive_isolate_cannot_publish_or_reset_primary_signal_authority() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                struct RestoreSigint(libc::sigaction);
                impl Drop for RestoreSigint {
                    fn drop(&mut self) {
                        unsafe {
                            libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut());
                        }
                    }
                }
                fn sigint_action() -> libc::sigaction {
                    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                    assert_eq!(
                        unsafe { libc::sigaction(libc::SIGINT, std::ptr::null(), &mut action) },
                        0
                    );
                    action
                }
                let _restore = RestoreSigint(sigint_action());
                let primary = runtime_state(py);
                assert!(async_work_owner(py));
                assert!(!UNHANDLED_KEYBOARD_INTERRUPT.load(Ordering::SeqCst));
                let handler = handler_object(py, record_delivery_handler as *const ());
                let old = molt_signal_signal(int_bits_from_i64(py, libc::SIGINT as i64), handler);
                assert!(!exception_pending(py));
                dec_ref_bits(py, old);
                let installed = sigint_action().sa_sigaction;
                assert_eq!(
                    installed,
                    molt_c_signal_handler as *const () as libc::sighandler_t
                );

                let mut isolate = Box::new(crate::state::runtime_state::RuntimeState::new());
                signal_record_delivery_for_test(py, libc::SIGINT);
                {
                    struct RestoreRuntime(*mut crate::state::runtime_state::RuntimeState);
                    impl Drop for RestoreRuntime {
                        fn drop(&mut self) {
                            crate::state::runtime_state::set_thread_runtime_state(self.0);
                        }
                    }
                    let _restore_runtime = RestoreRuntime(
                        (primary as *const crate::state::runtime_state::RuntimeState).cast_mut(),
                    );
                    crate::state::runtime_state::set_thread_runtime_state(&mut *isolate);
                    // Only identity/atomic paths are exercised under the empty
                    // isolate; it is never asked to construct a Python error.
                    assert!(signal_owner_thread());
                    assert!(!async_work_owner(py));
                    assert!(!signal_safepoint(py));
                    assert!(!signal_interrupt_occurred(py));
                    signal_note_unhandled_keyboard_interrupt(py);
                    assert!(!UNHANDLED_KEYBOARD_INTERRUPT.load(Ordering::SeqCst));
                    assert_eq!(
                        unsafe {
                            (molt_cpython_abi::hooks::hooks_or_stubs().attached_runtime_context)()
                        },
                        molt_cpython_abi::hooks::AttachedRuntimeContextKind::Detached as u32
                    );
                }
                signal_note_unhandled_keyboard_interrupt(py);
                assert!(!signal_runtime_state_publish(&isolate));
                assert!(UNHANDLED_KEYBOARD_INTERRUPT.load(Ordering::SeqCst));
                assert!(!signal_clear_state(py, &isolate));
                assert!(std::ptr::eq(
                    ACTIVE_SIGNAL_STATE.load(Ordering::SeqCst),
                    &primary.signal,
                ));
                assert_eq!(sigint_action().sa_sigaction, installed);
                assert!(signal_tripped());
                assert!(primary.signal.pending_for_test(libc::SIGINT));

                assert!(signal_clear_state(py, primary));
                assert!(ACTIVE_SIGNAL_STATE.load(Ordering::SeqCst).is_null());
                assert_eq!(sigint_action().sa_sigaction, libc::SIG_DFL);
                assert!(!signal_tripped());
                assert!(!signal_clear_state(py, primary));
                dec_ref_bits(py, handler);
            });
        });
        // A subsequent runtime gets fresh exit state through the sole
        // lifecycle publication boundary, not through handler updates.
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            assert!(!UNHANDLED_KEYBOARD_INTERRUPT.load(Ordering::SeqCst));
        });
    }

    #[cfg(unix)]
    #[test]
    fn wakeup_fd_rejects_blocking_or_truncated_descriptors_before_publication() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                use std::os::fd::AsRawFd;
                assert!(crate::has_capability(py, "signal.set_wakeup_fd"));
                assert!(async_work_owner(py));
                let (_reader, writer) = os_pipe::pipe().expect("pipe");
                let previous = runtime_state(py).signal.wakeup_fd();
                struct RestoreWakeupFd<'a>(&'a SignalRuntimeState, i32);
                impl Drop for RestoreWakeupFd<'_> {
                    fn drop(&mut self) {
                        self.0.swap_wakeup_fd(self.1);
                        quiesce_admitted_notifications();
                    }
                }
                // Restore and quiesce before the writer closes, including on panic.
                let _restore = RestoreWakeupFd(&runtime_state(py).signal, previous);
                let _ = molt_signal_set_wakeup_fd(int_bits_from_i64(py, writer.as_raw_fd() as i64));
                assert_eq!(take_pending_exception_kind(py), "ValueError");
                assert_eq!(runtime_state(py).signal.wakeup_fd(), previous);
                let _ = molt_signal_set_wakeup_fd(int_bits_from_i64(py, (i32::MAX as i64) + 1));
                assert_eq!(take_pending_exception_kind(py), "ValueError");
                assert_eq!(runtime_state(py).signal.wakeup_fd(), previous);
                let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
                assert!(flags >= 0);
                assert_eq!(
                    unsafe {
                        libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
                    },
                    0
                );
                let old =
                    molt_signal_set_wakeup_fd(int_bits_from_i64(py, writer.as_raw_fd() as i64));
                assert_eq!(to_i64(obj_from_bits(old)), Some(previous as i64));
                assert!(!exception_pending(py));
                assert_eq!(runtime_state(py).signal.wakeup_fd(), writer.as_raw_fd());
                let _ = molt_signal_set_wakeup_fd(int_bits_from_i64(py, previous as i64));
                assert!(!exception_pending(py));
                assert_eq!(runtime_state(py).signal.wakeup_fd(), previous);
            });
        });
    }

    #[test]
    fn wakeup_fd_requires_capability_before_descriptor_validation() {
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
            impl Drop for RestoreEnvironment {
                fn drop(&mut self) {
                    for (key, value) in &self.0 {
                        unsafe {
                            match value {
                                Some(value) => std::env::set_var(key, value),
                                None => std::env::remove_var(key),
                            }
                        }
                    }
                }
            }
            let _environment = RestoreEnvironment(
                ["MOLT_CAPABILITY_TIER", "MOLT_CAPABILITIES"]
                    .map(|key| (key, std::env::var_os(key)))
                    .into(),
            );
            unsafe {
                std::env::set_var("MOLT_CAPABILITY_TIER", "none");
                std::env::set_var("MOLT_CAPABILITIES", "");
            }
            assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
            crate::with_gil_entry_nopanic!(py, {
                assert!(!crate::has_capability(py, "signal.set_wakeup_fd"));
                assert!(async_work_owner(py));
                let previous = runtime_state(py).signal.wakeup_fd();
                let _ = molt_signal_set_wakeup_fd(int_bits_from_i64(py, -2));
                assert_eq!(take_pending_exception_kind(py), "PermissionError");
                assert_eq!(runtime_state(py).signal.wakeup_fd(), previous);
                assert!(!exception_pending(py));
            });
        });
    }

    #[test]
    fn dispatch_runs_handlers_in_signal_order_and_keeps_later_deliveries_after_a_raise() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let raising = handler_object(py, raising_handler as *const ());
            let recording = handler_object(py, record_delivery_handler as *const ());
            let (low, high) = (libc::SIGINT, libc::SIGTERM);
            let old_low = install(py, low, raising);
            let old_high = install(py, high, recording);
            RECORDED_SIGNUM.store(0, Ordering::SeqCst);
            signal_record_delivery_for_test(py, high);
            signal_record_delivery_for_test(py, low);
            assert!(
                signal_safepoint(py),
                "the lower signal's handler raises first"
            );
            assert_eq!(take_pending_exception_kind(py), "ValueError");
            assert_eq!(RECORDED_SIGNUM.load(Ordering::SeqCst), 0);
            assert!(signal_tripped(), "the later delivery must stay recorded");
            assert!(!signal_safepoint(py));
            assert_eq!(RECORDED_SIGNUM.load(Ordering::SeqCst), high as i64);
            assert!(RECORDED_FRAME_NONE.load(Ordering::SeqCst));
            assert!(!signal_tripped());
            restore(py, low, old_low);
            restore(py, high, old_high);
            dec_ref_bits(py, raising);
            dec_ref_bits(py, recording);
        });
    }

    #[test]
    fn default_int_disposition_dispatches_keyboard_interrupt() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sigint = libc::SIGINT;
            let old = install(py, sigint, HANDLER_DEFAULT_INT);
            assert_eq!(signal_set_interrupt(sigint as i64), 0);
            assert!(signal_tripped());
            assert!(signal_safepoint(py));
            assert_eq!(take_pending_exception_kind(py), "KeyboardInterrupt");
            assert!(!signal_tripped());
            restore(py, sigint, old);
        });
    }

    #[test]
    fn simulated_delivery_ignores_signals_python_does_not_handle() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sigterm = libc::SIGTERM;
            let old = install(py, sigterm, HANDLER_SIG_DFL);
            assert_eq!(signal_set_interrupt(sigterm as i64), 0);
            assert!(!signal_tripped(), "SIG_DFL is not handled by Python");
            let _ = install(py, sigterm, HANDLER_SIG_IGN);
            assert_eq!(signal_set_interrupt(sigterm as i64), 0);
            assert!(!signal_tripped(), "SIG_IGN is not handled by Python");
            assert_eq!(signal_set_interrupt(0), -1);
            assert_eq!(signal_set_interrupt(effective_nsig()), -1);
            restore(py, sigterm, old);
        });
    }

    #[test]
    fn interrupt_occurred_consumes_only_a_recorded_sigint() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sigint = libc::SIGINT;
            let old = install(py, sigint, HANDLER_DEFAULT_INT);
            assert!(!signal_interrupt_occurred(py));
            assert_eq!(signal_set_interrupt(sigint as i64), 0);
            assert!(signal_interrupt_occurred(py));
            assert!(!signal_interrupt_occurred(py));
            // The consumed delivery has no handler left to run.
            assert!(!signal_safepoint(py));
            assert!(!exception_pending(py));
            restore(py, sigint, old);
        });
    }

    #[test]
    fn a_propagating_exception_defers_dispatch_to_the_next_clean_safepoint() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sigint = libc::SIGINT;
            let old = install(py, sigint, HANDLER_DEFAULT_INT);
            let _ = raise_exception::<u64>(py, "RuntimeError", "already propagating");
            assert_eq!(signal_set_interrupt(sigint as i64), 0);
            assert!(
                !signal_safepoint(py),
                "no handler runs under a propagating exception"
            );
            assert_eq!(take_pending_exception_kind(py), "RuntimeError");
            assert!(signal_tripped());
            assert!(signal_safepoint(py));
            assert_eq!(take_pending_exception_kind(py), "KeyboardInterrupt");
            restore(py, sigint, old);
        });
    }

    #[test]
    fn non_owner_threads_never_dispatch() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sigint = libc::SIGINT;
            let old = install(py, sigint, HANDLER_DEFAULT_INT);
            assert_eq!(signal_set_interrupt(sigint as i64), 0);
            let observed = {
                let _released = crate::GilReleaseGuard::suspend();
                std::thread::spawn(|| {
                    crate::state::run_runtime_worker(|| {
                        crate::with_gil_entry_nopanic!(worker, {
                            let dispatched = signal_safepoint(worker);
                            (dispatched, exception_pending(worker), signal_owner_thread())
                        })
                    })
                })
                .join()
                .expect("worker")
            };
            assert_eq!(observed, (false, false, false));
            assert!(signal_tripped(), "the owner still has the delivery to run");
            assert!(signal_safepoint(py));
            assert_eq!(take_pending_exception_kind(py), "KeyboardInterrupt");
            restore(py, sigint, old);
        });
    }

    #[test]
    fn route_withdrawal_waits_for_every_admitted_delivery() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let parker = crate::async_rt::event_loop::LoopParker::new().expect("parker");
            let route = AsyncWorkParkRoute::publish(py, &parker).expect("nothing recorded");
            // A delivery admitted before the withdrawal and not yet finished.
            ASYNC_WORK_NOTIFIERS_IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
            let withdrawn = {
                let _released = crate::GilReleaseGuard::suspend();
                let withdrawer = std::thread::spawn(move || drop(route));
                std::thread::sleep(Duration::from_millis(100));
                let finished_early = withdrawer.is_finished();
                ASYNC_WORK_NOTIFIERS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
                withdrawer.join().expect("withdrawer");
                finished_early
            };
            assert!(
                !withdrawn,
                "the route was withdrawn while an admitted delivery could still use it"
            );
            assert!(
                runtime_state(py)
                    .signal
                    .park_route
                    .load(Ordering::SeqCst)
                    .is_null(),
                "withdrawal must unpublish the parker"
            );
        });
    }

    #[test]
    fn c_api_signal_projections_route_to_the_runtime_authority() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            use molt_cpython_abi::api::errors;
            let sigint = libc::SIGINT;
            let old = install(py, sigint, HANDLER_DEFAULT_INT);
            unsafe {
                assert_eq!(errors::PyErr_CheckSignals(), 0, "nothing is recorded");
                assert_eq!(errors::PyErr_SetInterruptEx(0), -1);
                errors::PyErr_SetInterrupt();
                assert_eq!(errors::PyOS_InterruptOccurred(), 1);
                assert_eq!(errors::PyOS_InterruptOccurred(), 0);
                // The consumed SIGINT leaves no handler to run.
                assert_eq!(errors::PyErr_CheckSignals(), 0);
                assert_eq!(errors::PyErr_SetInterruptEx(sigint), 0);
                assert_eq!(errors::PyErr_CheckSignals(), -1);
                assert_eq!(
                    errors::PyErr_Occurred(),
                    (&raw mut molt_cpython_abi::abi_types::PyExc_KeyboardInterrupt).cast()
                );
                errors::PyErr_Clear();
            }
            assert!(!exception_pending(py));
            assert!(!signal_tripped());
            restore(py, sigint, old);
        });
    }

    #[cfg(unix)]
    #[test]
    fn raw_handler_preserves_errno_when_its_wake_write_fails() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let recording = handler_object(py, record_delivery_handler as *const ());
            let sigusr2 = libc::SIGUSR2;
            let old = install(py, sigusr2, recording);
            let parker = crate::async_rt::event_loop::LoopParker::new().expect("parker");
            parker.fill_for_test();
            let route = AsyncWorkParkRoute::publish(py, &parker).expect("nothing recorded");
            raw_errno::set(libc::EDOM);
            deliver_raw_signal_for_test(sigusr2);
            assert_eq!(
                raw_errno::get(),
                libc::EDOM,
                "the interrupted code's errno changed under a failed wake write"
            );
            drop(route);
            assert!(signal_tripped());
            assert!(!signal_safepoint(py));
            assert_eq!(RECORDED_SIGNUM.load(Ordering::SeqCst), sigusr2 as i64);
            restore(py, sigusr2, old);
            dec_ref_bits(py, recording);
        });
    }

    #[cfg(unix)]
    #[test]
    fn delivery_records_pending_and_writes_the_python_wakeup_fd() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            use std::os::fd::AsRawFd;
            let sigusr2 = libc::SIGUSR2;
            let old_handler = install(py, sigusr2, HANDLER_DEFAULT_INT);
            let (reader, writer) = os_pipe::pipe().expect("pipe");
            let old_fd = runtime_state(py).signal.swap_wakeup_fd(writer.as_raw_fd());
            deliver_raw_signal_for_test(sigusr2);
            let mut byte = [0u8];
            let read = unsafe {
                libc::read(
                    reader.as_raw_fd(),
                    byte.as_mut_ptr().cast::<libc::c_void>(),
                    1,
                )
            };
            assert_eq!((read, byte[0]), (1, sigusr2 as u8));
            assert!(
                runtime_state(py).signal.pending_for_test(sigusr2),
                "the delivery a wakeup byte announces must be recorded"
            );
            runtime_state(py).signal.swap_wakeup_fd(old_fd);
            // Retire the recorded delivery (a DEFAULT_INT slot raises).
            assert!(signal_safepoint(py));
            let _ = take_pending_exception_kind(py);
            restore(py, sigusr2, old_handler);
        });
    }
}
