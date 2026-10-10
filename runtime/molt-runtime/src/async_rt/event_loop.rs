//! Molt Event Loop — Pure-Rust asyncio event loop core.
//!
//! This module implements the CPython 3.12+ `asyncio.BaseEventLoop` semantics
//! entirely in Rust, avoiding Molt method dispatch overhead in the hot path.
//!
//! Architecture:
//! - One ready FIFO of callback, Task step, and timer completion work per iteration
//! - Ordered timers with an ID index: O(log n) insert, fire, and cancel; no tombstones
//! - I/O registration: mio (native) or host-delegated (wasm32) reader/writer callbacks
//! - Registry state is GIL-serialized. An idle loop decides to park under the
//!   registry lock and blocks on its own parker with the GIL released; every
//!   publication that could end the wait claims the parked loop under that lock
//!   and signals it after releasing it (`EventLoopState::begin_park`).
//!
//! Cross-platform contract:
//! - Native (linux/macos/windows): mio epoll/kqueue/IOCP for I/O multiplexing
//! - WASM (wasi/browser): host-delegated poll via `molt_socket_poll_host`
//!
//! WASM compatibility:
//! - Timer/callback/ready-queue operations work on all targets (pure state machines).
//! - I/O fd-based operations (add_reader, add_writer, remove_reader, remove_writer,
//!   notify_reader_ready, notify_writer_ready) are gated with
//!   `#[cfg(not(target_arch = "wasm32"))]` — WASM has no fd-based I/O multiplexing.
//!   WASM stubs raise RuntimeError("operation not supported on WASM").
//! - `std::time::Instant` is used for monotonic timers. On wasm32-wasi this is
//!   backed by `clock_gettime(CLOCK_MONOTONIC)`. On wasm32-unknown-unknown it
//!   panics at runtime (Molt targets wasm32-wasi for WASM builds).
//!
//! All callbacks are u64 NaN-boxed Molt callable bits. The event loop invokes them
//! via `call_callable0` / `call_callable1` without leaving the Rust runtime.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::asyncio_call_method0;
use crate::{
    MoltObject, dec_ref_bits, exception_pending, inc_ref_bits, monotonic_now_secs, raise_exception,
    runtime_state,
};
#[path = "event_loop/park.rs"]
mod park;
#[path = "event_loop/pipe_transport.rs"]
mod pipe_transport;
pub(crate) use park::LoopParker;
pub(crate) use pipe_transport::PipeTransportRegistry;
pub use pipe_transport::{
    molt_pipe_transport_close, molt_pipe_transport_drop, molt_pipe_transport_get_fd,
    molt_pipe_transport_get_write_buffer_size, molt_pipe_transport_is_closing,
    molt_pipe_transport_new, molt_pipe_transport_pause_reading, molt_pipe_transport_resume_reading,
    molt_pipe_transport_write,
};

// --- State constants ---
const STATE_IDLE: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_CLOSED: u8 = 2;

// --- I/O registration entry ---

struct IoCallbackEntry {
    callback_bits: u64,
}

enum ReadyWork {
    Handle(u64),
    Task(super::scheduler::MoltTask),
    // A timer completion is a ready callback, not an inline Task step. It
    // schedules the continuation for the next snapshot, as Future completion does.
    WakeTask(super::scheduler::MoltTask),
}

impl ReadyWork {
    fn into_owned_bits(self) -> u64 {
        match self {
            Self::Handle(bits) => bits,
            Self::Task(task) | Self::WakeTask(task) => task.into_owned_bits(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum TimerKey {
    Handle(u64),
    Task(crate::PtrSlot),
}

fn timer_key(work: &ReadyWork, sequence: u64) -> TimerKey {
    match work {
        ReadyWork::Handle(_) => TimerKey::Handle(sequence),
        ReadyWork::Task(task) | ReadyWork::WakeTask(task) => {
            TimerKey::Task(crate::PtrSlot(task.future_ptr))
        }
    }
}

// --- Event loop state ---

/// Where the loop's driving thread is relative to its idle wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParkState {
    /// Not blocked: publishers need not signal.
    Running,
    /// Blocked, or about to block, until signalled or `deadline`.
    Parked { deadline: Option<Instant> },
    /// A publisher has already signalled the parked thread.
    Signalled,
}

/// Outcome of the atomic idle observation in `begin_park`.
enum ParkStep {
    /// Ready work, a due deadline, or a requested wake: run a turn.
    Proceed,
    /// Block on `parker` until signalled or `timeout` (None: no deadline).
    Block {
        parker: Arc<LoopParker>,
        timeout: Option<Duration>,
    },
    Closed,
}

struct EventLoopState {
    ready: VecDeque<ReadyWork>,
    timers: BTreeMap<(Instant, u64), ReadyWork>,
    timer_deadlines: HashMap<TimerKey, (Instant, u64)>,
    readers: HashMap<i64, IoCallbackEntry>,
    writers: HashMap<i64, IoCallbackEntry>,
    state: AtomicU8,
    timer_seq: AtomicU64,
    start_instant: Instant,
    debug: bool,
    exception_handler_bits: u64,
    task_factory_bits: u64,
    /// Created with the loop and released at close; publishers clone it so they
    /// can signal after dropping the registry lock.
    parker: Option<Arc<LoopParker>>,
    park: ParkState,
    /// A wake for state kept outside the queues (a `stop()` request).
    wake_requested: bool,
}

impl EventLoopState {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            ready: VecDeque::with_capacity(64),
            timers: BTreeMap::new(),
            timer_deadlines: HashMap::new(),
            readers: HashMap::new(),
            writers: HashMap::new(),
            state: AtomicU8::new(STATE_IDLE),
            timer_seq: AtomicU64::new(0),
            start_instant: Instant::now(),
            debug: false,
            exception_handler_bits: MoltObject::none().bits(),
            task_factory_bits: MoltObject::none().bits(),
            parker: Some(Arc::new(LoopParker::new()?)),
            park: ParkState::Running,
            wake_requested: false,
        })
    }

    #[inline]
    fn monotonic_secs(&self) -> f64 {
        self.start_instant.elapsed().as_secs_f64()
    }

    fn next_timer_seq(&self) -> u64 {
        self.timer_seq.fetch_add(1, Ordering::Relaxed)
    }

    fn is_running(&self) -> bool {
        self.state.load(Ordering::Relaxed) == STATE_RUNNING
    }

    fn is_closed(&self) -> bool {
        self.state.load(Ordering::Relaxed) == STATE_CLOSED
    }

    /// Claim the parked driving thread for new ready work. Only the first
    /// publisher after a park signals it; later ones find `Signalled`.
    fn claim_parked_for_ready(&mut self) -> Option<Arc<LoopParker>> {
        match self.park {
            ParkState::Parked { .. } => {
                self.park = ParkState::Signalled;
                self.parker.clone()
            }
            ParkState::Running | ParkState::Signalled => None,
        }
    }

    /// A new deadline needs the parked thread only when it precedes the one
    /// that thread is sleeping toward; a later one is seen at that wake.
    fn claim_parked_for_deadline(&mut self, deadline: Instant) -> Option<Arc<LoopParker>> {
        match self.park {
            ParkState::Parked {
                deadline: Some(parked),
            } if parked <= deadline => None,
            _ => self.claim_parked_for_ready(),
        }
    }

    /// Insert an owned callback timer; returns its cancellation id and the
    /// parked thread to signal when this deadline shortens its wait.
    fn insert_callback_timer(
        &mut self,
        py: &crate::PyToken<'_>,
        deadline: Instant,
        callback_bits: u64,
    ) -> (u64, Option<Arc<LoopParker>>) {
        let seq = self.next_timer_seq();
        inc_ref_bits(py, callback_bits);
        self.timers
            .insert((deadline, seq), ReadyWork::Handle(callback_bits));
        self.timer_deadlines
            .insert(TimerKey::Handle(seq), (deadline, seq));
        (seq, self.claim_parked_for_deadline(deadline))
    }

    /// Observe, under the registry lock, whether the loop can run a turn or
    /// must park. Every publication that could change the answer takes the
    /// same lock and claims a parked loop, so no wake falls between this
    /// observation and the block: a publication either precedes it (and is
    /// seen here) or finds `Parked` and posts the parker's sticky token.
    fn begin_park(&mut self, now: Instant) -> ParkStep {
        if self.is_closed() {
            return ParkStep::Closed;
        }
        if std::mem::take(&mut self.wake_requested) || !self.ready.is_empty() {
            return ParkStep::Proceed;
        }
        let deadline = self
            .timers
            .first_key_value()
            .map(|(&(deadline, _), _)| deadline);
        if deadline.is_some_and(|deadline| deadline <= now) {
            return ParkStep::Proceed;
        }
        let Some(parker) = self.parker.clone() else {
            return ParkStep::Closed;
        };
        self.park = ParkState::Parked { deadline };
        ParkStep::Block {
            parker,
            timeout: deadline.map(|deadline| deadline.saturating_duration_since(now)),
        }
    }

    fn end_park(&mut self) {
        self.park = ParkState::Running;
    }
}

// --- Runtime-owned handle registry (cross-thread safe, GIL-serialized) ---

pub(crate) struct EventLoopRegistry {
    loops: Mutex<HashMap<u64, EventLoopState>>,
    next_handle: AtomicU64,
}

impl EventLoopRegistry {
    pub(crate) fn new() -> Self {
        Self {
            loops: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        }
    }

    fn alloc_loop(&self) -> std::io::Result<u64> {
        // The wake pipe is acquired before the loop becomes visible, so a
        // descriptor-exhausted process fails at construction, as CPython's
        // self-pipe does, never in the middle of a run.
        let state = EventLoopState::new()?;
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        self.loops.lock().unwrap().insert(handle, state);
        Ok(handle)
    }

    pub(crate) fn clear(&self, _py: &crate::PyToken<'_>) -> bool {
        let loops = {
            let mut guard = self.loops.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        // Callbacks released below can allocate another loop. Never reuse an
        // identity while stale handles from the detached cohort can still run.
        let changed = !loops.is_empty();
        for mut state in loops.into_values() {
            // A thread still parked on a retired loop wakes, finds no loop and
            // raises, instead of sleeping on a pipe nothing can signal again.
            if let Some(parker) = state.claim_parked_for_ready() {
                parker.unpark();
            }
            release_event_loop_state_refs(_py, state);
        }
        changed
    }
}

fn drain_event_loop_state_refs(state: &mut EventLoopState) -> Vec<u64> {
    let mut refs = Vec::new();
    for work in state.ready.drain(..) {
        refs.push(work.into_owned_bits());
    }
    refs.extend(
        std::mem::take(&mut state.timers)
            .into_values()
            .map(|work| work.into_owned_bits()),
    );
    state.timer_deadlines.clear();
    for (_, entry) in state.readers.drain() {
        refs.push(entry.callback_bits);
    }
    for (_, entry) in state.writers.drain() {
        refs.push(entry.callback_bits);
    }
    refs.push(std::mem::replace(
        &mut state.exception_handler_bits,
        MoltObject::none().bits(),
    ));
    refs.push(std::mem::replace(
        &mut state.task_factory_bits,
        MoltObject::none().bits(),
    ));
    refs
}

fn release_event_loop_state_refs(_py: &crate::PyToken<'_>, mut state: EventLoopState) {
    for bits in drain_event_loop_state_refs(&mut state) {
        dec_ref_bits(_py, bits);
    }
}

/// Extract the raw event loop handle from potentially NaN-boxed bits.
/// `alloc_loop` returns a plain u64 counter, but `molt_event_loop_new`
/// wraps it as `MoltObject::from_int(handle)`.  Every intrinsic receives
/// the NaN-boxed form from Python, so we must unbox before registry lookup.
#[inline(always)]
fn unbox_loop_handle(handle: u64) -> u64 {
    let obj = MoltObject::from_bits(handle);
    if obj.is_int() {
        obj.as_int_unchecked() as u64
    } else {
        handle
    }
}

#[inline]
fn event_loop_registry(_py: &crate::PyToken<'_>) -> &'static EventLoopRegistry {
    &runtime_state(_py).event_loop_registry
}

fn with_loop<F, R>(_py: &crate::PyToken<'_>, handle: u64, f: F) -> Option<R>
where
    F: FnOnce(&mut EventLoopState) -> R,
{
    let key = unbox_loop_handle(handle);
    let mut map = event_loop_registry(_py).loops.lock().unwrap();
    map.get_mut(&key).map(f)
}

/// Signal a parked loop claimed under the registry lock. Called only after
/// that lock is released: `unpark` is a syscall and must not extend it.
#[inline]
fn signal_claimed(parked: Option<Arc<LoopParker>>) {
    if let Some(parker) = parked {
        parker.unpark();
    }
}

fn cancel_io_handle(_py: &crate::PyToken<'_>, bits: u64) {
    unsafe {
        let result = asyncio_call_method0(_py, bits, b"cancel");
        if !crate::obj_from_bits(result).is_none() {
            dec_ref_bits(_py, result);
        }
    }
    dec_ref_bits(_py, bits);
}

/// The ready stream owns one reference for each queued Task step, just as for
/// Handles. Spawn-retain protects suspended tasks; queue custody independently
/// protects a step across completion/finalization while it is being dispatched.
pub(super) fn enqueue_loop_task(
    _py: &crate::PyToken<'_>,
    loop_handle: u64,
    task: super::scheduler::MoltTask,
) -> Result<(), super::scheduler::MoltTask> {
    let mut task = Some(task);
    let Some((accepted, parked)) = with_loop(_py, loop_handle, |state| {
        if state.is_closed() {
            return (false, None);
        }
        state.ready.push_back(ReadyWork::Task(task.take().unwrap()));
        (true, state.claim_parked_for_ready())
    }) else {
        return Err(task.take().unwrap());
    };
    signal_claimed(parked);
    if accepted {
        Ok(())
    } else {
        Err(task.take().unwrap())
    }
}

/// All loop-owned delays share callback clock/order and one cancellation index.
pub(super) fn register_loop_sleep(
    py: &crate::PyToken<'_>,
    loop_handle: u64,
    task_ptr: *mut u8,
    deadline: Instant,
) {
    let parked = with_loop(py, loop_handle, |state| {
        let key = TimerKey::Task(crate::PtrSlot(task_ptr));
        if state.is_closed() || state.timer_deadlines.contains_key(&key) {
            return None;
        }
        let ordered = (deadline, state.next_timer_seq());
        state.timers.insert(
            ordered,
            ReadyWork::WakeTask(super::scheduler::MoltTask::new(py, task_ptr)),
        );
        state.timer_deadlines.insert(key, ordered);
        state.claim_parked_for_deadline(deadline)
    })
    .flatten();
    signal_claimed(parked);
}

/// Transfer the timer's owned edge to the caller for release outside registry locks.
pub(super) fn take_loop_sleep(
    py: &crate::PyToken<'_>,
    loop_handle: u64,
    task_ptr: *mut u8,
) -> Option<u64> {
    with_loop(py, loop_handle, |state| {
        let ordered = state
            .timer_deadlines
            .remove(&TimerKey::Task(crate::PtrSlot(task_ptr)))?;
        state
            .timers
            .remove(&ordered)
            .map(|work| work.into_owned_bits())
    })
    .flatten()
}

pub(super) fn loop_task_sleep_scheduled(
    py: &crate::PyToken<'_>,
    loop_handle: u64,
    task_ptr: *mut u8,
) -> bool {
    with_loop(py, loop_handle, |state| {
        state
            .timer_deadlines
            .contains_key(&TimerKey::Task(crate::PtrSlot(task_ptr)))
    })
    .unwrap_or(false)
}

/// Bind scheduling ownership before the Task's first spawn. The existing token
/// holds the immutable loop identity, so every subsequent wake uses the same FIFO.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_spawn(loop_handle: u64, task_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if with_loop(_py, loop_handle, |state| !state.is_closed()) != Some(true) {
            return raise_exception::<u64>(_py, "RuntimeError", "Event loop is closed");
        }
        let Some(task_ptr) = crate::resolve_task_ptr(task_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        unsafe {
            let flags = (*crate::header_from_obj_ptr(task_ptr)).load_synchronized_flags();
            if flags
                & (crate::HEADER_FLAG_SPAWN_RETAIN
                    | crate::HEADER_FLAG_TASK_QUEUED
                    | crate::HEADER_FLAG_TASK_RUNNING
                    | crate::HEADER_FLAG_TASK_DONE)
                != 0
            {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "Task runner was already started",
                );
            }
        }
        if let Err(message) = super::cancellation::bind_task_loop(_py, task_ptr, loop_handle) {
            return raise_exception::<u64>(_py, "RuntimeError", message);
        }
        unsafe { super::scheduler::molt_spawn(task_bits) }
    })
}

// --- Intrinsics ---

/// Create a new event loop. Returns a handle (u64).
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match event_loop_registry(_py).alloc_loop() {
            Ok(handle) => MoltObject::from_int(handle as i64).bits(),
            Err(err) => crate::raise_os_error::<u64>(_py, err, "event loop wake pipe"),
        }
    })
}

/// Enqueue a callback for immediate execution (next iteration).
/// This is the fast path — no lock acquire/release Python method calls,
/// just a direct VecDeque push. Any thread may publish: a parked loop is
/// signalled, which is what makes `call_soon_threadsafe` wake the loop.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_call_soon(loop_handle: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(published) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return Err(());
            }
            inc_ref_bits(_py, callback_bits);
            state.ready.push_back(ReadyWork::Handle(callback_bits));
            Ok(state.claim_parked_for_ready())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        let Ok(parked) = published else {
            return raise_exception::<u64>(_py, "RuntimeError", "Event loop is closed");
        };
        signal_claimed(parked);
        MoltObject::none().bits()
    })
}

/// Return a callback timer's id, signalling a parked loop whose wait it shortens.
type TimerRegistrationResult = Option<Result<(u64, Option<Arc<LoopParker>>), ()>>;

fn timer_registration_result(py: &crate::PyToken<'_>, timer: TimerRegistrationResult) -> u64 {
    match timer {
        Some(Ok((id, parked))) => {
            signal_claimed(parked);
            MoltObject::from_int(id as i64).bits()
        }
        Some(Err(())) => raise_exception::<u64>(py, "OverflowError", "timer deadline out of range"),
        None => raise_exception::<u64>(py, "RuntimeError", "event loop is closed"),
    }
}

/// Schedule a callback after `delay_secs` seconds.
/// Returns a timer ID that can be used for cancellation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_call_later(
    loop_handle: u64,
    delay_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let delay_obj = crate::obj_from_bits(delay_bits);
        let delay_secs = delay_obj
            .as_float()
            .unwrap_or_else(|| crate::to_i64(delay_obj).map(|i| i as f64).unwrap_or(0.0));
        let Some(timer) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return None;
            }
            let Some(deadline) =
                Instant::now().checked_add(Duration::from_nanos((delay_secs * 1e9) as u64))
            else {
                return Some(Err(()));
            };
            Some(Ok(state.insert_callback_timer(
                _py,
                deadline,
                callback_bits,
            )))
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        timer_registration_result(_py, timer)
    })
}

/// Schedule a callback at absolute time `when_secs` (monotonic clock).
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_call_at(
    loop_handle: u64,
    when_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let when_obj = crate::obj_from_bits(when_bits);
        let when_secs = when_obj
            .as_float()
            .unwrap_or_else(|| crate::to_i64(when_obj).map(|i| i as f64).unwrap_or(0.0));
        let Some(timer) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return None;
            }
            let Some(deadline) = state
                .start_instant
                .checked_add(Duration::from_nanos((when_secs * 1e9) as u64))
            else {
                return Some(Err(()));
            };
            Some(Ok(state.insert_callback_timer(
                _py,
                deadline,
                callback_bits,
            )))
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        timer_registration_result(_py, timer)
    })
}

/// Cancel a timer and release its callback immediately, outside the registry lock.
/// The deadline index removes both scheduling entries in O(log n); canceled long
/// timers never leave retained handles or heap tombstones until their deadline.
/// A parked loop is not signalled: it wakes once at the old deadline, finds
/// nothing due, and parks toward the next one.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_cancel_timer(loop_handle: u64, timer_id_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let timer_id = crate::to_i64(crate::obj_from_bits(timer_id_bits)).unwrap_or(-1) as u64;
        let Some(callback) = with_loop(_py, loop_handle, |state| {
            let ordered = state.timer_deadlines.remove(&TimerKey::Handle(timer_id))?;
            state
                .timers
                .remove(&ordered)
                .map(|work| work.into_owned_bits())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        if let Some(bits) = callback {
            dec_ref_bits(_py, bits);
        }
        MoltObject::from_bool(callback.is_some()).bits()
    })
}

/// Register a file descriptor for read readiness notification.
///
/// Not available on WASM — file descriptor I/O multiplexing is unsupported.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_add_reader(
    loop_handle: u64,
    fd_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        let Some(previous) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return None;
            }
            inc_ref_bits(_py, callback_bits);
            state
                .readers
                .insert(fd, IoCallbackEntry { callback_bits })
                .map(|old| old.callback_bits)
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        if let Some(previous) = previous {
            cancel_io_handle(_py, previous);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_add_reader(
    _loop_handle: u64,
    _fd_bits: u64,
    _callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "add_reader not supported on WASM")
    })
}

/// Remove a file descriptor's read readiness callback.
///
/// Not available on WASM — file descriptor I/O multiplexing is unsupported.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_remove_reader(loop_handle: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        let Some(removed) = with_loop(_py, loop_handle, |state| {
            state.readers.remove(&fd).map(|entry| entry.callback_bits)
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        if let Some(bits) = removed {
            cancel_io_handle(_py, bits);
        }
        MoltObject::from_bool(removed.is_some()).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_remove_reader(_loop_handle: u64, _fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "remove_reader not supported on WASM")
    })
}

/// Register a file descriptor for write readiness notification.
///
/// Not available on WASM — file descriptor I/O multiplexing is unsupported.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_add_writer(
    loop_handle: u64,
    fd_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        let Some(previous) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return None;
            }
            inc_ref_bits(_py, callback_bits);
            state
                .writers
                .insert(fd, IoCallbackEntry { callback_bits })
                .map(|old| old.callback_bits)
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        if let Some(previous) = previous {
            cancel_io_handle(_py, previous);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_add_writer(
    _loop_handle: u64,
    _fd_bits: u64,
    _callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "add_writer not supported on WASM")
    })
}

/// Remove a file descriptor's write readiness callback.
///
/// Not available on WASM — file descriptor I/O multiplexing is unsupported.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_remove_writer(loop_handle: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        let Some(removed) = with_loop(_py, loop_handle, |state| {
            state.writers.remove(&fd).map(|entry| entry.callback_bits)
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        if let Some(bits) = removed {
            cancel_io_handle(_py, bits);
        }
        MoltObject::from_bool(removed.is_some()).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_remove_writer(_loop_handle: u64, _fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "remove_writer not supported on WASM")
    })
}

/// Execute one turn of the loop's sole callback queue. Tasks may enqueue Handles;
/// due timers join those Handles before the batch is captured. Anything scheduled
/// by a callback belongs to the next turn, regardless of the active loop driver.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_run_once(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(closed) = with_loop(_py, loop_handle, |state| state.is_closed()) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        if closed {
            return MoltObject::from_int(0).bits();
        }
        #[cfg(target_arch = "wasm32")]
        runtime_state(_py).io_poller().poll_host(_py);
        let Some(mut batch) = with_loop(_py, loop_handle, |state| {
            let now = Instant::now();
            while let Some((&(deadline, sequence), _)) = state.timers.first_key_value() {
                if deadline > now {
                    break;
                }
                let (_, work) = state.timers.pop_first().unwrap();
                state.timer_deadlines.remove(&timer_key(&work, sequence));
                state.ready.push_back(work);
            }
            std::mem::take(&mut state.ready)
        }) else {
            return MoltObject::from_int(0).bits();
        };
        let mut callbacks_run = 0;
        while let Some(work) = batch.pop_front() {
            match work {
                ReadyWork::Handle(handle) => unsafe {
                    run_event_loop_handle(_py, handle);
                    dec_ref_bits(_py, handle);
                },
                ReadyWork::Task(task) => runtime_state(_py).scheduler().execute_loop_task(task),
                ReadyWork::WakeTask(task) => super::scheduler::wake_task_ptr(_py, task.future_ptr),
            }
            callbacks_run += 1;
            if exception_pending(_py) {
                // Handle._run reports ordinary callback errors. Fatal exceptions
                // escape the driver; remaining batch ownership survives restart.
                with_loop(_py, loop_handle, |state| {
                    batch.append(&mut state.ready);
                    std::mem::swap(&mut state.ready, &mut batch);
                });
                for remaining in batch {
                    dec_ref_bits(_py, remaining.into_owned_bits());
                }
                return MoltObject::none().bits();
            }
        }
        MoltObject::from_int(callbacks_run).bits()
    })
}

/// Handle._run owns callback context and exception reporting. Never clear a
/// propagated fatal exception here or silently discard the rest of its batch.
unsafe fn run_event_loop_handle(_py: &crate::PyToken<'_>, handle_bits: u64) {
    unsafe {
        let result = asyncio_call_method0(_py, handle_bits, b"_run");
        if !crate::obj_from_bits(result).is_none() {
            dec_ref_bits(_py, result);
        }
    }
}

/// Get the current monotonic time of the event loop (seconds, float).
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_time(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(time) = with_loop(_py, loop_handle, |state| state.monotonic_secs()) else {
            return MoltObject::from_float(monotonic_now_secs(_py)).bits();
        };
        MoltObject::from_float(time).bits()
    })
}

/// Park the driving thread until ready work, the loop's earliest deadline, a
/// requested wake, close, or recorded signal/C pending-call work (active
/// process-main owner only). Native parks release the GIL and hold no runtime lock.
///
/// Python signal handlers never run here: a delivery ends the wait, and the
/// generated eval-breaker safepoint after this call returns dispatches them
/// (`signal_ext::signal_safepoint`), as CPython does after `select` returns.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_wait(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let async_work_owner = crate::builtins::signal_ext::async_work_owner(_py);
        loop {
            if async_work_owner && crate::builtins::signal_ext::async_work_pending() {
                return MoltObject::none().bits();
            }
            let Some(step) = with_loop(_py, loop_handle, |state| state.begin_park(Instant::now()))
            else {
                return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
            };
            let (parker, timeout) = match step {
                ParkStep::Proceed => return MoltObject::none().bits(),
                ParkStep::Closed => {
                    return raise_exception::<u64>(_py, "RuntimeError", "Event loop is closed");
                }
                ParkStep::Block { parker, timeout } => (parker, timeout),
            };
            let parked = parker.park(_py, timeout, async_work_owner);
            with_loop(_py, loop_handle, |state| state.end_park());
            if let Err(err) = parked {
                return crate::raise_os_error::<u64>(_py, err, "event loop wait");
            }
        }
    })
}

/// Wake the loop for state kept outside its queues (a `stop()` request). The
/// request is consumed by the next idle observation, so a racing park cannot
/// lose it. `stop()` never raises in CPython: a retired handle is a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_wake(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let parked = with_loop(_py, loop_handle, |state| {
            state.wake_requested = true;
            state.claim_parked_for_ready()
        })
        .flatten();
        signal_claimed(parked);
        MoltObject::none().bits()
    })
}

/// Start the event loop (set state to running).
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_start(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(()) = with_loop(_py, loop_handle, |state| {
            state.state.store(STATE_RUNNING, Ordering::Relaxed);
            // A stop wake left by an earlier run does not end this run's waits.
            state.wake_requested = false;
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::none().bits()
    })
}

/// Stop the event loop (set state to idle).
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_stop(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(()) = with_loop(_py, loop_handle, |state| {
            let current = state.state.load(Ordering::Relaxed);
            if current == STATE_RUNNING {
                state.state.store(STATE_IDLE, Ordering::Relaxed);
            }
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::none().bits()
    })
}

/// Check if the event loop is currently running.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_is_running(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(running) = with_loop(_py, loop_handle, |state| state.is_running()) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(running).bits()
    })
}

/// Check if the event loop is closed.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_is_closed(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(closed) = with_loop(_py, loop_handle, |state| state.is_closed()) else {
            return MoltObject::from_bool(true).bits();
        };
        MoltObject::from_bool(closed).bits()
    })
}

/// Close the event loop. Cleans up all pending callbacks and I/O registrations,
/// releases the wake parker, and wakes a thread still parked on the loop so it
/// observes the close instead of sleeping on a released pipe.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_close(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let loop_key = unbox_loop_handle(loop_handle);
        let (callbacks_to_free, parked, parker) = {
            let mut map = event_loop_registry(_py).loops.lock().unwrap();
            let Some(state) = map.get_mut(&loop_key) else {
                return MoltObject::none().bits();
            };
            if state.is_closed() {
                return MoltObject::none().bits();
            }
            state.state.store(STATE_CLOSED, Ordering::Relaxed);
            let parked = state.claim_parked_for_ready();
            (
                drain_event_loop_state_refs(state),
                parked,
                state.parker.take(),
            )
        };
        // The pipe closes with its last reference, outside the registry lock;
        // a parked thread keeps its own reference until it wakes.
        signal_claimed(parked);
        drop(parker);
        let spawn_roots = super::cancellation::take_loop_spawn_roots(_py, loop_handle);
        // All registries/flags are detached before callback-bearing releases.
        for cb in callbacks_to_free.into_iter().chain(spawn_roots) {
            dec_ref_bits(_py, cb);
        }
        MoltObject::none().bits()
    })
}

/// Drop the event loop handle. Closes if not already closed, then removes from registry.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_drop(loop_handle: u64) -> u64 {
    // Close first to ensure proper cleanup.
    molt_event_loop_close(loop_handle);
    let loop_key = unbox_loop_handle(loop_handle);
    crate::with_gil_entry_nopanic!(_py, {
        let removed = event_loop_registry(_py)
            .loops
            .lock()
            .unwrap()
            .remove(&loop_key);
        // State teardown runs outside the registry lock.
        drop(removed);
        MoltObject::none().bits()
    })
}

/// Set the event loop's debug mode.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_set_debug(loop_handle: u64, enabled_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let enabled = crate::is_truthy(_py, crate::obj_from_bits(enabled_bits));
        let Some(()) = with_loop(_py, loop_handle, |state| {
            state.debug = enabled;
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::none().bits()
    })
}

/// Get the event loop's debug mode.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_get_debug(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(debug) = with_loop(_py, loop_handle, |state| state.debug) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(debug).bits()
    })
}

/// Set the event loop's exception handler callback.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_set_exception_handler(
    loop_handle: u64,
    handler_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(old) = with_loop(_py, loop_handle, |state| {
            let old = state.exception_handler_bits;
            inc_ref_bits(_py, handler_bits);
            state.exception_handler_bits = handler_bits;
            old
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        dec_ref_bits(_py, old);
        MoltObject::none().bits()
    })
}

/// Get the event loop's exception handler callback.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_get_exception_handler(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(bits) = with_loop(_py, loop_handle, |state| {
            inc_ref_bits(_py, state.exception_handler_bits);
            state.exception_handler_bits
        }) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

/// Set the event loop's task factory callback.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_set_task_factory(loop_handle: u64, factory_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(old) = with_loop(_py, loop_handle, |state| {
            let old = state.task_factory_bits;
            inc_ref_bits(_py, factory_bits);
            state.task_factory_bits = factory_bits;
            old
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        dec_ref_bits(_py, old);
        MoltObject::none().bits()
    })
}

/// Get the event loop's task factory callback.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_get_task_factory(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(bits) = with_loop(_py, loop_handle, |state| {
            inc_ref_bits(_py, state.task_factory_bits);
            state.task_factory_bits
        }) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

/// Notify the event loop that a file descriptor is ready for reading.
/// Called by the IoPoller when I/O readiness is detected.
/// Moves the reader's callback to the ready queue for execution.
///
/// Not available on WASM — file descriptor I/O multiplexing is unsupported.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_notify_reader_ready(loop_handle: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        let parked = with_loop(_py, loop_handle, |state| {
            let callback = state.readers.get(&fd)?.callback_bits;
            inc_ref_bits(_py, callback);
            state.ready.push_back(ReadyWork::Handle(callback));
            state.claim_parked_for_ready()
        })
        .flatten();
        signal_claimed(parked);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_notify_reader_ready(_loop_handle: u64, _fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "notify_reader_ready not supported on WASM",
        )
    })
}

/// Notify the event loop that a file descriptor is ready for writing.
///
/// Not available on WASM — file descriptor I/O multiplexing is unsupported.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_notify_writer_ready(loop_handle: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        let parked = with_loop(_py, loop_handle, |state| {
            let callback = state.writers.get(&fd)?.callback_bits;
            inc_ref_bits(_py, callback);
            state.ready.push_back(ReadyWork::Handle(callback));
            state.claim_parked_for_ready()
        })
        .flatten();
        signal_claimed(parked);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_notify_writer_ready(_loop_handle: u64, _fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "notify_writer_ready not supported on WASM",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MoltObject, alloc_string, header_from_obj_ptr};

    fn ref_count(ptr: *mut u8) -> u32 {
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn event_loop_close_releases_all_callback_roots() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = alloc_string(_py, b"event-loop-retained-callback");
            let bits = MoltObject::from_ptr(ptr).bits();
            let initial_refs = ref_count(ptr);

            let loop_handle = molt_event_loop_new();
            let _ = molt_event_loop_call_soon(loop_handle, bits);
            let _ = molt_event_loop_set_exception_handler(loop_handle, bits);
            let _ = molt_event_loop_set_task_factory(loop_handle, bits);
            assert_eq!(ref_count(ptr), initial_refs + 3);

            let _ = molt_event_loop_close(loop_handle);
            assert_eq!(ref_count(ptr), initial_refs);

            let _ = molt_event_loop_drop(loop_handle);
            dec_ref_bits(_py, bits);
        });
    }

    #[test]
    fn cancelled_long_timers_release_callback_and_index_immediately() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = alloc_string(_py, b"long-timeout-callback");
            let bits = MoltObject::from_ptr(ptr).bits();
            let initial_refs = ref_count(ptr);
            let loop_handle = molt_event_loop_new();
            for _ in 0..100_000 {
                let timer = molt_event_loop_call_later(
                    loop_handle,
                    MoltObject::from_float(86_400.0).bits(),
                    bits,
                );
                assert_eq!(ref_count(ptr), initial_refs + 1);
                assert_eq!(
                    molt_event_loop_cancel_timer(loop_handle, timer),
                    MoltObject::from_bool(true).bits()
                );
                assert_eq!(ref_count(ptr), initial_refs);
                assert_eq!(
                    molt_event_loop_cancel_timer(loop_handle, timer),
                    MoltObject::from_bool(false).bits()
                );
            }
            with_loop(_py, loop_handle, |state| {
                assert!(state.timers.is_empty());
                assert!(state.timer_deadlines.is_empty());
            })
            .unwrap();
            molt_event_loop_drop(loop_handle);
            dec_ref_bits(_py, bits);
        });
    }
}

#[cfg(test)]
mod task_timer_tests {
    use super::*;

    #[test]
    fn loop_task_timer_cancel_and_close_release_owned_reference() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let task = crate::molt_future_new(0, 0);
            let task_ptr = crate::ptr_from_bits(task);
            let refs = || unsafe { (*crate::header_from_obj_ptr(task_ptr)).ref_count_snapshot() };
            let initial = refs();
            let owner = molt_event_loop_new();
            let deadline = Instant::now() + Duration::from_secs(86400);
            register_loop_sleep(py, owner, task_ptr, deadline);
            register_loop_sleep(py, owner, task_ptr, deadline);
            assert_eq!(refs(), initial + 1);
            assert!(loop_task_sleep_scheduled(py, owner, task_ptr));
            assert_eq!(take_loop_sleep(py, owner, task_ptr), Some(task));
            assert!(!loop_task_sleep_scheduled(py, owner, task_ptr));
            assert_eq!(take_loop_sleep(py, owner, task_ptr), None);
            dec_ref_bits(py, task);
            assert_eq!(refs(), initial);
            with_loop(py, owner, |state| {
                assert!(state.timers.is_empty());
                assert!(state.timer_deadlines.is_empty());
            })
            .unwrap();
            register_loop_sleep(py, owner, task_ptr, deadline);
            assert_eq!(refs(), initial + 1);
            molt_event_loop_close(owner);
            assert_eq!(refs(), initial);
            molt_event_loop_drop(owner);
            dec_ref_bits(py, task);
        });
    }
}

/// Park/wake protocol tests. They drive `begin_park`/`end_park` and the parker
/// directly, so each test controls the exact interleaving it proves; timing
/// bounds are generous (a lost wake stalls for the full far deadline).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod park_tests {
    use super::*;
    use crate::alloc_string;

    const FAR: Duration = Duration::from_secs(30);
    const PROMPT: Duration = Duration::from_secs(5);

    fn begin(py: &crate::PyToken<'_>, handle: u64) -> ParkStep {
        with_loop(py, handle, |state| state.begin_park(Instant::now())).expect("live loop")
    }

    fn park_state(py: &crate::PyToken<'_>, handle: u64) -> ParkState {
        with_loop(py, handle, |state| state.park).expect("live loop")
    }

    fn end(py: &crate::PyToken<'_>, handle: u64) {
        with_loop(py, handle, |state| state.end_park());
    }

    fn object(py: &crate::PyToken<'_>, label: &[u8]) -> u64 {
        MoltObject::from_ptr(alloc_string(py, label)).bits()
    }

    fn seconds(value: f64) -> u64 {
        MoltObject::from_float(value).bits()
    }

    fn retire(py: &crate::PyToken<'_>, handle: u64, objects: &[u64]) {
        molt_event_loop_close(handle);
        molt_event_loop_drop(handle);
        for &bits in objects {
            dec_ref_bits(py, bits);
        }
    }

    fn take_pending_exception(py: &crate::PyToken<'_>) {
        assert!(exception_pending(py), "expected a pending exception");
        let raised = crate::molt_exception_last();
        crate::molt_exception_clear();
        dec_ref_bits(py, raised);
    }

    #[test]
    fn idle_loop_parks_until_its_earliest_deadline_without_a_cap() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let far = object(py, b"far");
            let _ = molt_event_loop_call_later(handle, seconds(60.0), far);
            match begin(py, handle) {
                ParkStep::Block {
                    timeout: Some(timeout),
                    ..
                } => assert!(
                    timeout > Duration::from_secs(59),
                    "idle wait capped at {timeout:?}"
                ),
                _ => panic!("an idle loop with only a far timer must park until it"),
            }
            end(py, handle);
            retire(py, handle, &[far]);
        });
    }

    #[test]
    fn publication_after_park_registration_ends_the_block_at_once() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let ready = object(py, b"ready");
            let ParkStep::Block {
                parker,
                timeout: None,
            } = begin(py, handle)
            else {
                panic!("an empty loop parks without a deadline");
            };
            // The publisher runs after the park decision and before the block.
            let _ = molt_event_loop_call_soon(handle, ready);
            assert_eq!(park_state(py, handle), ParkState::Signalled);
            let started = Instant::now();
            parker.park(py, Some(FAR), false).expect("park");
            assert!(
                started.elapsed() < PROMPT,
                "ready publication lost its wake"
            );
            end(py, handle);
            assert!(matches!(begin(py, handle), ParkStep::Proceed));
            retire(py, handle, &[ready]);
        });
    }

    #[test]
    fn a_second_publication_after_a_claim_issues_no_second_token() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let first = object(py, b"first");
            let second = object(py, b"second");
            let ParkStep::Block { parker, .. } = begin(py, handle) else {
                panic!("an empty loop parks");
            };
            let _ = molt_event_loop_call_soon(handle, first);
            let _ = molt_event_loop_call_soon(handle, second);
            parker.park(py, Some(FAR), false).expect("park");
            end(py, handle);
            // One claim, one token: the drained parker holds no stale token.
            assert!(
                !parker.token_pending(),
                "duplicate wake token after one claim"
            );
            assert!(matches!(begin(py, handle), ParkStep::Proceed));
            retire(py, handle, &[first, second]);
        });
    }

    #[test]
    fn only_an_earlier_deadline_signals_a_parked_loop() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let timer = object(py, b"timer");
            let _ = molt_event_loop_call_later(handle, seconds(60.0), timer);
            let ParkStep::Block { parker, .. } = begin(py, handle) else {
                panic!("expected a timed park");
            };
            let _ = molt_event_loop_call_later(handle, seconds(120.0), timer);
            assert!(
                matches!(park_state(py, handle), ParkState::Parked { .. }),
                "a later deadline must leave the parked loop asleep"
            );
            let _ = molt_event_loop_call_later(handle, seconds(0.5), timer);
            assert_eq!(park_state(py, handle), ParkState::Signalled);
            let started = Instant::now();
            parker.park(py, Some(FAR), false).expect("park");
            assert!(started.elapsed() < PROMPT);
            end(py, handle);
            match begin(py, handle) {
                ParkStep::Block {
                    timeout: Some(timeout),
                    ..
                } => assert!(timeout <= Duration::from_millis(500)),
                ParkStep::Proceed => {}
                _ => panic!("the earlier deadline must bound the next park"),
            }
            end(py, handle);
            retire(py, handle, &[timer]);
        });
    }

    #[test]
    fn an_earlier_task_sleep_signals_a_parked_loop() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let task = crate::molt_future_new(0, 0);
            let task_ptr = crate::ptr_from_bits(task);
            let far = object(py, b"far");
            let _ = molt_event_loop_call_later(handle, seconds(60.0), far);
            let ParkStep::Block { .. } = begin(py, handle) else {
                panic!("expected a timed park");
            };
            register_loop_sleep(
                py,
                handle,
                task_ptr,
                Instant::now() + Duration::from_secs(1),
            );
            assert_eq!(
                park_state(py, handle),
                ParkState::Signalled,
                "a task sleep shorter than the parked deadline must wake the loop"
            );
            end(py, handle);
            retire(py, handle, &[far, task]);
        });
    }

    #[test]
    fn requested_wake_is_consumed_by_exactly_one_wait() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let _ = molt_event_loop_wake(handle);
            assert!(matches!(begin(py, handle), ParkStep::Proceed));
            assert!(matches!(begin(py, handle), ParkStep::Block { .. }));
            end(py, handle);
            // Starting a run discards a stale stop wake.
            let _ = molt_event_loop_wake(handle);
            let _ = molt_event_loop_start(handle);
            assert!(matches!(begin(py, handle), ParkStep::Block { .. }));
            end(py, handle);
            let _ = molt_event_loop_stop(handle);
            retire(py, handle, &[]);
        });
    }

    #[test]
    fn waking_a_retired_handle_is_a_no_op() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            retire(py, handle, &[]);
            let _ = molt_event_loop_wake(handle);
            assert!(
                !exception_pending(py),
                "stop() on a retired loop must not raise"
            );
        });
    }

    #[test]
    fn close_releases_a_parked_loop_and_the_wait_reports_closed() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let ParkStep::Block { parker, .. } = begin(py, handle) else {
                panic!("an empty loop parks");
            };
            let _ = molt_event_loop_close(handle);
            let started = Instant::now();
            parker.park(py, Some(FAR), false).expect("park");
            assert!(started.elapsed() < PROMPT);
            end(py, handle);
            let _ = molt_event_loop_wait(handle);
            take_pending_exception(py);
            with_loop(py, handle, |state| assert!(state.parker.is_none())).expect("live loop");
            retire(py, handle, &[]);
        });
    }

    #[test]
    fn registry_clear_wakes_a_loop_parked_on_the_retired_registry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let registry = EventLoopRegistry::new();
            let handle = registry.alloc_loop().expect("wake pipe");
            let parker = {
                let mut loops = registry.loops.lock().unwrap();
                match loops
                    .get_mut(&handle)
                    .expect("fresh loop")
                    .begin_park(Instant::now())
                {
                    ParkStep::Block { parker, .. } => parker,
                    _ => panic!("an empty loop parks"),
                }
            };
            assert!(registry.clear(py));
            assert!(
                parker.token_pending(),
                "clearing the registry must wake a parked loop"
            );
        });
    }

    #[test]
    fn cross_thread_publication_ends_a_blocked_park_before_its_far_deadline() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let far = object(py, b"far");
            let ready = object(py, b"cross-thread");
            let _ = molt_event_loop_call_later(handle, seconds(30.0), far);
            let publisher = std::thread::spawn(move || {
                crate::state::run_runtime_worker(|| {
                    std::thread::sleep(Duration::from_millis(50));
                    let _ = molt_event_loop_call_soon(handle, ready);
                })
            });
            let started = Instant::now();
            while with_loop(py, handle, |state| state.ready.is_empty()).expect("live loop") {
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "blocked park missed a cross-thread wake"
                );
                match begin(py, handle) {
                    // Releases the GIL while blocked, so the publisher can run.
                    ParkStep::Block { parker, timeout } => {
                        parker.park(py, timeout, false).expect("park");
                        end(py, handle);
                    }
                    ParkStep::Proceed | ParkStep::Closed => break,
                }
            }
            assert!(
                started.elapsed() < PROMPT,
                "cross-thread publication was not prompt"
            );
            assert_eq!(with_loop(py, handle, |state| state.ready.len()), Some(1));
            {
                let _released = crate::GilReleaseGuard::suspend();
                publisher.join().expect("publisher");
            }
            retire(py, handle, &[far, ready]);
        });
    }

    #[test]
    fn cross_thread_task_wake_ends_a_blocked_park() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let task = crate::molt_future_new(0, 0);
            // Transfer the scheduler's admitted Send handle, not a raw-pointer
            // field captured separately by the closure. The caller's task
            // reference stays live until the worker has joined.
            let scheduled = super::super::scheduler::MoltTask::new(py, crate::ptr_from_bits(task));
            let waker = std::thread::spawn(move || {
                crate::state::run_runtime_worker(|| {
                    std::thread::sleep(Duration::from_millis(50));
                    crate::with_gil_entry_nopanic!(worker_py, {
                        assert!(enqueue_loop_task(worker_py, handle, scheduled).is_ok());
                    });
                })
            });
            let ParkStep::Block { parker, timeout } = begin(py, handle) else {
                panic!("an empty loop parks");
            };
            assert_eq!(timeout, None);
            let started = Instant::now();
            parker.park(py, Some(FAR), false).expect("park");
            end(py, handle);
            assert!(
                started.elapsed() < PROMPT,
                "task wake from a worker was lost"
            );
            {
                let _released = crate::GilReleaseGuard::suspend();
                waker.join().expect("waker");
            }
            assert!(matches!(begin(py, handle), ParkStep::Proceed));
            // Close releases the ready queue's reference to the task.
            retire(py, handle, &[task]);
        });
    }

    static C_PENDING_CALLBACKS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "C" fn record_c_pending_call(_arg: *mut std::ffi::c_void) -> std::os::raw::c_int {
        C_PENDING_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        0
    }

    fn assert_pending_call_drained_once(py: &crate::PyToken<'_>, handle: u64) {
        use molt_cpython_abi::api::pending_calls::has_pending_calls;
        assert!(has_pending_calls());
        assert!(!crate::builtins::signal_ext::signal_tripped());
        assert_eq!(C_PENDING_CALLBACKS.load(Ordering::SeqCst), 0);
        // No ready callback or loop deadline: the C queue alone makes the
        // public wait return to its generated eval-breaker safepoint.
        assert_eq!(molt_event_loop_wait(handle), MoltObject::none().bits());
        assert!(!exception_pending(py));
        assert_eq!(crate::molt_async_work_poll_and_exception_pending(), 0);
        assert!(!has_pending_calls());
        assert_eq!(C_PENDING_CALLBACKS.load(Ordering::SeqCst), 1);
        assert_eq!(crate::molt_async_work_poll_and_exception_pending(), 0);
        assert_eq!(C_PENDING_CALLBACKS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn c_pending_call_published_before_park_returns_to_canonical_safepoint() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            C_PENDING_CALLBACKS.store(0, Ordering::SeqCst);
            let handle = molt_event_loop_new();
            let ParkStep::Block { parker, .. } = begin(py, handle) else {
                panic!("an empty loop parks");
            };
            assert_eq!(
                unsafe {
                    molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                        Some(record_c_pending_call),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            let started = Instant::now();
            parker.park(py, Some(FAR), true).expect("park");
            assert!(
                started.elapsed() < PROMPT,
                "queued C work must prevent blocking"
            );
            end(py, handle);
            assert_pending_call_drained_once(py, handle);
            retire(py, handle, &[]);
        });
    }

    #[test]
    fn c_pending_call_published_to_armed_owner_wakes_without_loop_work() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            C_PENDING_CALLBACKS.store(0, Ordering::SeqCst);
            let handle = molt_event_loop_new();
            let ParkStep::Block { parker, .. } = begin(py, handle) else {
                panic!("an empty loop parks");
            };
            let publisher = std::thread::spawn(|| {
                let started = Instant::now();
                while !crate::builtins::signal_ext::async_work_route_published_for_test() {
                    assert!(
                        started.elapsed() < PROMPT,
                        "owner never armed its wake route"
                    );
                    std::thread::yield_now();
                }
                // No runtime attachment, GIL, loop-ready callback, or timer.
                unsafe {
                    molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                        Some(record_c_pending_call),
                        std::ptr::null_mut(),
                    )
                }
            });
            let started = Instant::now();
            parker.park(py, Some(FAR), true).expect("park");
            end(py, handle);
            let publication = {
                let _released = crate::GilReleaseGuard::suspend();
                publisher.join().expect("publisher")
            };
            assert_eq!(publication, 0);
            assert!(
                started.elapsed() < PROMPT,
                "C publication lost its park wake"
            );
            assert_pending_call_drained_once(py, handle);
            retire(py, handle, &[]);
        });
    }

    #[cfg(unix)]
    #[test]
    fn signal_owner_park_returns_without_blocking_on_a_recorded_delivery() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let ParkStep::Block { parker, .. } = begin(py, handle) else {
                panic!("an empty loop parks");
            };
            crate::builtins::signal_ext::signal_record_delivery_for_test(py, libc::SIGUSR2);
            let started = Instant::now();
            parker.park(py, Some(FAR), true).expect("park");
            assert!(
                started.elapsed() < PROMPT,
                "a recorded delivery must end a signal-owner park"
            );
            end(py, handle);
            // The wait reports the delivery instead of parking again.
            let _ = molt_event_loop_wait(handle);
            assert!(!exception_pending(py));
            // No Python handler is installed, so the safepoint only retires it.
            assert!(!crate::builtins::signal_ext::signal_safepoint(py));
            assert!(!crate::builtins::signal_ext::signal_tripped());
            retire(py, handle, &[]);
        });
    }

    #[cfg(unix)]
    #[test]
    fn raw_delivery_on_another_thread_wakes_the_owner_park() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let handle = molt_event_loop_new();
            let ParkStep::Block { parker, .. } = begin(py, handle) else {
                panic!("an empty loop parks");
            };
            let deliverer = std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                // Exactly what the kernel runs on the receiving thread.
                crate::builtins::signal_ext::deliver_raw_signal_for_test(libc::SIGUSR2);
            });
            let started = Instant::now();
            parker.park(py, Some(FAR), true).expect("park");
            end(py, handle);
            assert!(
                started.elapsed() < PROMPT,
                "a delivery on another thread must reach the parked owner"
            );
            {
                let _released = crate::GilReleaseGuard::suspend();
                deliverer.join().expect("deliverer");
            }
            assert!(crate::builtins::signal_ext::signal_tripped());
            assert!(!crate::builtins::signal_ext::signal_safepoint(py));
            retire(py, handle, &[]);
        });
    }
}
