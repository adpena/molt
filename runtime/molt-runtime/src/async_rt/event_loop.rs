//! Molt Event Loop — Pure-Rust asyncio event loop core.
//!
//! This module implements the CPython 3.12+ `asyncio.BaseEventLoop` semantics
//! entirely in Rust, avoiding Molt method dispatch overhead in the hot path.
//!
//! Architecture:
//! - ReadyQueue: VecDeque<u64> of callback bits, drained per iteration
//! - TimerHeap: BinaryHeap<TimerEntry> for call_later/call_at (min-heap by deadline)
//! - I/O registration: mio (native) or host-delegated (wasm32) reader/writer callbacks
//! - Single-threaded under GIL; Mutex is for Rust Send/Sync, never contended
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

use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicI64;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use super::asyncio_call_method0;
use crate::{
    MoltObject, dec_ref_bits, exception_pending, inc_ref_bits, monotonic_now_secs, raise_exception,
    runtime_state,
};

#[cfg(windows)]
#[inline]
fn libc_write_count(len: usize) -> libc::c_uint {
    len.min(u32::MAX as usize) as libc::c_uint
}

#[cfg(not(windows))]
#[inline]
fn libc_write_count(len: usize) -> usize {
    len
}

// --- State constants ---
const STATE_IDLE: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_CLOSED: u8 = 2;

// --- Timer entry (min-heap by deadline, FIFO tiebreak) ---

#[derive(Clone)]
struct TimerEntry {
    deadline_ns: u64,
    sequence: u64,
    callback_bits: u64,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline_ns == other.deadline_ns && self.sequence == other.sequence
    }
}

impl Eq for TimerEntry {}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Min-heap: reverse ordering so smallest deadline is popped first.
        // Tiebreak by sequence (FIFO: smaller sequence first).
        other
            .deadline_ns
            .cmp(&self.deadline_ns)
            .then(other.sequence.cmp(&self.sequence))
    }
}

// --- I/O registration entry ---

struct IoCallbackEntry {
    callback_bits: u64,
}

// --- Event loop state ---

struct EventLoopState {
    ready: VecDeque<u64>,
    timers: BinaryHeap<TimerEntry>,
    cancelled_timers: HashSet<u64>,
    readers: HashMap<i64, IoCallbackEntry>,
    writers: HashMap<i64, IoCallbackEntry>,
    state: AtomicU8,
    timer_seq: AtomicU64,
    start_instant: Instant,
    debug: bool,
    exception_handler_bits: u64,
    task_factory_bits: u64,
}

impl EventLoopState {
    fn new() -> Self {
        Self {
            ready: VecDeque::with_capacity(64),
            timers: BinaryHeap::with_capacity(32),
            cancelled_timers: HashSet::new(),
            readers: HashMap::new(),
            writers: HashMap::new(),
            state: AtomicU8::new(STATE_IDLE),
            timer_seq: AtomicU64::new(0),
            start_instant: Instant::now(),
            debug: false,
            exception_handler_bits: MoltObject::none().bits(),
            task_factory_bits: MoltObject::none().bits(),
        }
    }

    #[inline]
    fn monotonic_ns(&self) -> u64 {
        self.start_instant.elapsed().as_nanos() as u64
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

    fn alloc_loop(&self) -> u64 {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        self.loops
            .lock()
            .unwrap()
            .insert(handle, EventLoopState::new());
        handle
    }

    pub(crate) fn clear(&self, _py: &crate::PyToken<'_>) {
        let loops = {
            let mut guard = self.loops.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for state in loops.into_values() {
            release_event_loop_state_refs(_py, state);
        }
        self.next_handle.store(1, Ordering::Relaxed);
    }
}

fn drain_event_loop_state_refs(state: &mut EventLoopState) -> Vec<u64> {
    let mut refs = Vec::new();
    for bits in state.ready.drain(..) {
        refs.push(bits);
    }
    while let Some(entry) = state.timers.pop() {
        refs.push(entry.callback_bits);
    }
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

// --- Intrinsics ---

/// Create a new event loop. Returns a handle (u64).
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = event_loop_registry(_py).alloc_loop();
        MoltObject::from_int(handle as i64).bits()
    })
}

/// Enqueue a callback for immediate execution (next iteration).
/// This is the fast path — no lock acquire/release Python method calls,
/// just a direct VecDeque push.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_call_soon(loop_handle: u64, callback_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = loop_handle;
        let Some(()) = with_loop(_py, handle, |state| {
            if state.is_closed() {
                return;
            }
            inc_ref_bits(_py, callback_bits);
            state.ready.push_back(callback_bits);
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::none().bits()
    })
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
        if delay_secs <= 0.0 {
            return molt_event_loop_call_soon(loop_handle, callback_bits);
        }
        let Some(timer_id) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return None;
            }
            let deadline_ns = state.monotonic_ns() + (delay_secs * 1e9) as u64;
            let seq = state.next_timer_seq();
            inc_ref_bits(_py, callback_bits);
            state.timers.push(TimerEntry {
                deadline_ns,
                sequence: seq,
                callback_bits,
            });
            Some(seq)
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        match timer_id {
            Some(id) => MoltObject::from_int(id as i64).bits(),
            None => raise_exception::<u64>(_py, "RuntimeError", "event loop is closed"),
        }
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
        let Some(timer_id) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return None;
            }
            let now_secs = state.monotonic_secs();
            let delay = (when_secs - now_secs).max(0.0);
            let deadline_ns = state.monotonic_ns() + (delay * 1e9) as u64;
            let seq = state.next_timer_seq();
            inc_ref_bits(_py, callback_bits);
            state.timers.push(TimerEntry {
                deadline_ns,
                sequence: seq,
                callback_bits,
            });
            Some(seq)
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        match timer_id {
            Some(id) => MoltObject::from_int(id as i64).bits(),
            None => raise_exception::<u64>(_py, "RuntimeError", "event loop is closed"),
        }
    })
}

/// Cancel a scheduled timer by its sequence ID.
/// Uses an O(1) HashSet lookup during timer drain to skip cancelled entries.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_cancel_timer(loop_handle: u64, timer_id_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let timer_id = crate::to_i64(crate::obj_from_bits(timer_id_bits)).unwrap_or(-1) as u64;
        let Some(cancelled) = with_loop(_py, loop_handle, |state| {
            state.cancelled_timers.insert(timer_id)
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::from_bool(cancelled).bits()
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
        let Some(()) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return;
            }
            // Remove old reader callback for this fd if any.
            if let Some(old) = state.readers.remove(&fd) {
                dec_ref_bits(_py, old.callback_bits);
            }
            inc_ref_bits(_py, callback_bits);
            state.readers.insert(fd, IoCallbackEntry { callback_bits });
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
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
            if let Some(old) = state.readers.remove(&fd) {
                dec_ref_bits(_py, old.callback_bits);
                true
            } else {
                false
            }
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::from_bool(removed).bits()
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
        let Some(()) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return;
            }
            if let Some(old) = state.writers.remove(&fd) {
                dec_ref_bits(_py, old.callback_bits);
            }
            inc_ref_bits(_py, callback_bits);
            state.writers.insert(fd, IoCallbackEntry { callback_bits });
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
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
            if let Some(old) = state.writers.remove(&fd) {
                dec_ref_bits(_py, old.callback_bits);
                true
            } else {
                false
            }
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::from_bool(removed).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_remove_writer(_loop_handle: u64, _fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "remove_writer not supported on WASM")
    })
}

/// Execute one iteration of the event loop:
/// 1. Drain ready queue → invoke all callbacks
/// 2. Check timer heap → pop expired timers → invoke their callbacks
/// 3. Poll I/O → invoke reader/writer callbacks for ready fds
///
/// Returns the number of callbacks executed.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_run_once(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut callbacks_run: i64 = 0;

        let loop_key = unbox_loop_handle(loop_handle);
        let registry = event_loop_registry(_py);
        {
            let map = registry.loops.lock().unwrap();
            let Some(state) = map.get(&loop_key) else {
                return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
            };
            if state.is_closed() {
                return MoltObject::from_int(0).bits();
            }
        }

        // Phase 0: Advance the coroutine scheduler by one drain so that tasks
        // created via ``create_task``/``ensure_future`` make progress within the
        // same run_forever turn that CPython's ``_run_once`` would give them.
        runtime_state(_py).scheduler().drain_ready();

        // Phase 1: Run the ready Handles scheduled via ``call_soon`` (snapshotting
        // the current contents so callbacks scheduled by these handles run on the
        // next turn, matching CPython's ``ntodo = len(self._ready)`` semantics).
        let ready_batch: Vec<u64> = {
            let mut map = registry.loops.lock().unwrap();
            let Some(state) = map.get_mut(&loop_key) else {
                return MoltObject::from_int(callbacks_run).bits();
            };
            if state.is_closed() {
                return MoltObject::from_int(callbacks_run).bits();
            }
            state.ready.drain(..).collect()
        };
        for handle_bits in &ready_batch {
            unsafe {
                run_event_loop_handle(_py, *handle_bits);
            }
            dec_ref_bits(_py, *handle_bits);
            callbacks_run += 1;
        }

        // Phase 2: Pop expired timers and run their Handles.
        let now_ns = {
            let map = registry.loops.lock().unwrap();
            let Some(state) = map.get(&loop_key) else {
                return MoltObject::from_int(callbacks_run).bits();
            };
            state.monotonic_ns()
        };
        loop {
            let entry: Option<TimerEntry> = {
                let mut map = registry.loops.lock().unwrap();
                let Some(state) = map.get_mut(&loop_key) else {
                    break;
                };
                if let Some(top) = state.timers.peek() {
                    if top.deadline_ns <= now_ns {
                        state.timers.pop()
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            let Some(entry) = entry else {
                break;
            };
            // Check O(1) cancelled set.
            let is_cancelled = {
                let map = registry.loops.lock().unwrap();
                map.get(&loop_key)
                    .is_some_and(|s| s.cancelled_timers.contains(&entry.sequence))
            };
            if is_cancelled {
                // Remove from cancelled set to prevent unbounded growth.
                if let Some(state) = registry.loops.lock().unwrap().get_mut(&loop_key) {
                    state.cancelled_timers.remove(&entry.sequence);
                }
                dec_ref_bits(_py, entry.callback_bits);
                continue;
            }
            unsafe {
                run_event_loop_handle(_py, entry.callback_bits);
            }
            dec_ref_bits(_py, entry.callback_bits);
            callbacks_run += 1;
        }

        // Phase 3: I/O polling is handled by the existing IoPoller integration.
        // The event loop's readers/writers map provides fd→callback for
        // when the IoPoller reports readiness via the existing infrastructure.
        // We enqueue reader/writer callbacks to the ready queue when I/O is ready,
        // which will be drained on the next run_once iteration.

        MoltObject::from_int(callbacks_run).bits()
    })
}

/// Run a single ``call_soon``/timer ``Handle`` by invoking its ``_run`` method,
/// matching CPython's ``Handle._run`` dispatch. Exceptions raised by the
/// callback are reported through the loop's exception handler contract by the
/// Python ``Handle._run`` wrapper; any that still propagate here are cleared so
/// one bad callback cannot abort the whole event-loop turn.
///
/// # Safety
/// - `handle_bits` must be a valid asyncio ``Handle`` object exposing ``_run``.
unsafe fn run_event_loop_handle(_py: &crate::PyToken<'_>, handle_bits: u64) {
    unsafe {
        let result = asyncio_call_method0(_py, handle_bits, b"_run");
        if exception_pending(_py) {
            // Swallow handler exceptions per asyncio contract; log in debug mode.
            crate::builtins::exceptions::clear_exception(_py);
        }
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

/// Get the delay until the next scheduled timer fires (or -1.0 if none).
/// Used by the scheduler to determine how long to sleep.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_next_deadline_delay(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(delay) = with_loop(_py, loop_handle, |state| {
            let Some(top) = state.timers.peek() else {
                return -1.0f64;
            };
            let now_ns = state.monotonic_ns();
            if top.deadline_ns <= now_ns {
                0.0f64
            } else {
                (top.deadline_ns - now_ns) as f64 / 1e9
            }
        }) else {
            return MoltObject::from_float(-1.0).bits();
        };
        MoltObject::from_float(delay).bits()
    })
}

/// Check if the ready queue or timer heap has pending work.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_has_pending(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(has) = with_loop(_py, loop_handle, |state| {
            !state.ready.is_empty()
                || state
                    .timers
                    .peek()
                    .is_some_and(|t| t.deadline_ns <= state.monotonic_ns())
        }) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(has).bits()
    })
}

/// Get the number of pending callbacks in the ready queue.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_ready_count(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(count) = with_loop(_py, loop_handle, |state| state.ready.len() as i64) else {
            return MoltObject::from_int(0).bits();
        };
        MoltObject::from_int(count).bits()
    })
}

/// Start the event loop (set state to running).
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_start(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(()) = with_loop(_py, loop_handle, |state| {
            state.state.store(STATE_RUNNING, Ordering::Relaxed);
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

/// Close the event loop. Cleans up all pending callbacks and I/O registrations.
#[unsafe(no_mangle)]
pub extern "C" fn molt_event_loop_close(loop_handle: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let loop_key = unbox_loop_handle(loop_handle);
        let callbacks_to_free: Vec<u64> = {
            let mut map = event_loop_registry(_py).loops.lock().unwrap();
            let Some(state) = map.get_mut(&loop_key) else {
                return MoltObject::none().bits();
            };
            if state.is_closed() {
                return MoltObject::none().bits();
            }
            state.state.store(STATE_CLOSED, Ordering::Relaxed);
            drain_event_loop_state_refs(state)
        };
        // Dec-ref all freed callbacks outside the lock.
        for cb in callbacks_to_free {
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
        let mut map = event_loop_registry(_py).loops.lock().unwrap();
        map.remove(&loop_key);
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
        let Some(bits) = with_loop(_py, loop_handle, |state| state.exception_handler_bits) else {
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
        let Some(bits) = with_loop(_py, loop_handle, |state| state.task_factory_bits) else {
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
        let Some(cb_opt) = with_loop(_py, loop_handle, |state| {
            state.readers.get(&fd).map(|e| e.callback_bits)
        }) else {
            return MoltObject::none().bits();
        };
        if let Some(cb) = cb_opt {
            inc_ref_bits(_py, cb);
            with_loop(_py, loop_handle, |state| {
                state.ready.push_back(cb);
            });
        }
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
        let Some(cb_opt) = with_loop(_py, loop_handle, |state| {
            state.writers.get(&fd).map(|e| e.callback_bits)
        }) else {
            return MoltObject::none().bits();
        };
        if let Some(cb) = cb_opt {
            inc_ref_bits(_py, cb);
            with_loop(_py, loop_handle, |state| {
                state.ready.push_back(cb);
            });
        }
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

// ============================================================================
// Pipe Transport — fd-based read/write transports for asyncio.connect_read_pipe
// and asyncio.connect_write_pipe.
//
// Architecture:
// - PipeTransportState: per-handle state for a pipe transport (fd, direction,
//   closing/paused flags, write buffer).
// - Handle registry: runtime-owned Mutex<HashMap<i64, PipeTransportState>> with
//   atomic counter for handle allocation (same pattern as event loop handles).
// - Native targets: full fd-based I/O via libc read/write.
// - WASM targets: all pipe transport operations return error sentinels since
//   WASM does not support file descriptors in the traditional sense.
// ============================================================================

/// Internal state for a single pipe transport instance.
struct PipeTransportState {
    /// The underlying file descriptor.
    fd: i32,
    /// True for read pipes, false for write pipes.
    is_read: bool,
    /// Whether close() has been called.
    closing: bool,
    /// Whether reading is paused (read pipes only).
    paused: bool,
    /// Buffered writes pending flush (write pipes only).
    write_buffer: VecDeque<Vec<u8>>,
}

impl PipeTransportState {
    fn new(fd: i32, is_read: bool) -> Self {
        Self {
            fd,
            is_read,
            closing: false,
            paused: false,
            write_buffer: VecDeque::new(),
        }
    }
}

/// Runtime-owned pipe transport handle registry.
pub(crate) struct PipeTransportRegistry {
    transports: Mutex<HashMap<i64, PipeTransportState>>,
    #[cfg(not(target_arch = "wasm32"))]
    next_handle: AtomicI64,
}

impl PipeTransportRegistry {
    pub(crate) fn new() -> Self {
        Self {
            transports: Mutex::new(HashMap::new()),
            #[cfg(not(target_arch = "wasm32"))]
            next_handle: AtomicI64::new(1),
        }
    }

    pub(crate) fn clear(&self) {
        let transports = {
            let mut guard = self.transports.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            for (_, mut state) in transports {
                close_pipe_transport_state(&mut state);
            }
            self.next_handle.store(1, Ordering::Relaxed);
        }
        #[cfg(target_arch = "wasm32")]
        drop(transports);
    }
}

#[inline]
fn pipe_transport_registry(_py: &crate::PyToken<'_>) -> &'static PipeTransportRegistry {
    &runtime_state(_py).pipe_transport_registry
}

#[cfg(not(target_arch = "wasm32"))]
fn alloc_pipe_transport(_py: &crate::PyToken<'_>, fd: i32, is_read: bool) -> i64 {
    let registry = pipe_transport_registry(_py);
    let handle = registry.next_handle.fetch_add(1, Ordering::Relaxed);
    registry
        .transports
        .lock()
        .unwrap()
        .insert(handle, PipeTransportState::new(fd, is_read));
    handle
}

fn with_pipe<F, R>(_py: &crate::PyToken<'_>, handle: i64, f: F) -> Option<R>
where
    F: FnOnce(&mut PipeTransportState) -> R,
{
    let mut map = pipe_transport_registry(_py).transports.lock().unwrap();
    map.get_mut(&handle).map(f)
}

/// Extract a bytes-like slice from NaN-boxed bits.
/// Returns Ok(slice) or Err(exception sentinel bits).
#[cfg(not(target_arch = "wasm32"))]
fn pipe_require_bytes_slice(_py: &crate::PyToken<'_>, bits: u64) -> Result<&'static [u8], u64> {
    let obj = crate::obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "a bytes-like object is required",
        ));
    };
    unsafe {
        if let Some(slice) = crate::object::memoryview::bytes_like_slice(ptr) {
            return Ok(slice);
        }
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "a bytes-like object is required",
    ))
}

// --- Pipe transport intrinsics ---

/// Create a new pipe transport wrapping a file descriptor.
///
/// `fd_bits`: NaN-boxed integer file descriptor.
/// `is_read_bits`: NaN-boxed integer (truthy = read pipe, falsy = write pipe).
///
/// Returns a NaN-boxed integer handle for the pipe transport.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_pipe_transport_new(fd_bits: u64, is_read_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        let is_read = crate::is_truthy(_py, crate::obj_from_bits(is_read_bits));
        let handle = alloc_pipe_transport(_py, fd as i32, is_read);
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_pipe_transport_new(_fd_bits: u64, _is_read_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "pipe transports are not supported on WASM",
        )
    })
}

/// Get the file descriptor from a pipe transport.
///
/// Returns a NaN-boxed integer fd, or -1 if the handle is invalid.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_get_fd(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(fd) = with_pipe(_py, handle, |state| state.fd as i64) else {
            return MoltObject::from_int(-1).bits();
        };
        MoltObject::from_int(fd).bits()
    })
}

/// Check if the pipe transport is closing.
///
/// Returns a NaN-boxed bool.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_is_closing(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(closing) = with_pipe(_py, handle, |state| state.closing) else {
            return MoltObject::from_bool(true).bits();
        };
        MoltObject::from_bool(closing).bits()
    })
}

/// Close the pipe transport.
///
/// Marks the transport as closing and closes the underlying fd.
/// For write pipes, any buffered data is flushed first.
/// Returns None.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_pipe_transport_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let mut map = pipe_transport_registry(_py).transports.lock().unwrap();
        if let Some(state) = map.get_mut(&handle) {
            close_pipe_transport_state(state);
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn close_pipe_transport_state(state: &mut PipeTransportState) {
    if state.closing {
        return;
    }
    state.closing = true;
    if !state.is_read {
        let fd = state.fd;
        for chunk in state.write_buffer.drain(..) {
            let mut offset = 0usize;
            while offset < chunk.len() {
                let rc = unsafe {
                    libc::write(
                        fd as libc::c_int,
                        chunk[offset..].as_ptr() as *const libc::c_void,
                        libc_write_count(chunk.len() - offset),
                    )
                };
                if rc <= 0 {
                    break;
                }
                offset += rc as usize;
            }
        }
    }
    unsafe {
        libc::close(state.fd as libc::c_int);
    }
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_pipe_transport_close(_handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "pipe transports are not supported on WASM",
        )
    })
}

/// Pause reading on a read pipe transport.
///
/// Returns None. Raises RuntimeError if the transport is a write pipe.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_pause_reading(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(result) = with_pipe(_py, handle, |state| {
            if !state.is_read {
                return Err("pause_reading() called on write pipe transport");
            }
            if state.closing {
                return Err("transport is closing");
            }
            state.paused = true;
            Ok(())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "pipe transport not found");
        };
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "RuntimeError", msg),
        }
    })
}

/// Resume reading on a read pipe transport.
///
/// Returns None. Raises RuntimeError if the transport is a write pipe.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_resume_reading(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(result) = with_pipe(_py, handle, |state| {
            if !state.is_read {
                return Err("resume_reading() called on write pipe transport");
            }
            if state.closing {
                return Err("transport is closing");
            }
            state.paused = false;
            Ok(())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "pipe transport not found");
        };
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "RuntimeError", msg),
        }
    })
}

/// Write data to a write pipe transport.
///
/// `data_bits`: NaN-boxed bytes object.
///
/// The data is written directly to the fd if possible; any remainder that would
/// block is buffered internally. Returns None.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_pipe_transport_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        // Extract bytes from the data object.
        let data = match pipe_require_bytes_slice(_py, data_bits) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        if data.is_empty() {
            return MoltObject::none().bits();
        }
        let Some(result) = with_pipe(_py, handle, |state| {
            if state.is_read {
                return Err("write() called on read pipe transport");
            }
            if state.closing {
                return Err("transport is closing");
            }
            // Try to write directly first; buffer remainder.
            let fd = state.fd;
            let mut offset = 0usize;
            while offset < data.len() {
                let rc = unsafe {
                    libc::write(
                        fd as libc::c_int,
                        data[offset..].as_ptr() as *const libc::c_void,
                        libc_write_count(data.len() - offset),
                    )
                };
                if rc < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::Interrupted
                    {
                        // Buffer remaining data for later flush.
                        state.write_buffer.push_back(data[offset..].to_vec());
                        return Ok(());
                    }
                    // Other error — buffer and let protocol handle it.
                    state.write_buffer.push_back(data[offset..].to_vec());
                    return Ok(());
                }
                offset += rc as usize;
            }
            Ok(())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "pipe transport not found");
        };
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "RuntimeError", msg),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_pipe_transport_write(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "pipe transports are not supported on WASM",
        )
    })
}

/// Get the write buffer size for a pipe transport.
///
/// Returns a NaN-boxed integer (total bytes buffered).
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_get_write_buffer_size(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(size) = with_pipe(_py, handle, |state| {
            state
                .write_buffer
                .iter()
                .map(|chunk| chunk.len())
                .sum::<usize>() as i64
        }) else {
            return MoltObject::from_int(0).bits();
        };
        MoltObject::from_int(size).bits()
    })
}

/// Drop a pipe transport handle, removing it from the registry.
/// If the transport is not yet closed, it is closed first (native only).
/// On WASM, simply removes from the registry (no fd to close).
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_drop(handle_bits: u64) {
    // Close first to flush any pending writes and release the fd.
    // On WASM, skip close since pipe transports cannot be created there.
    #[cfg(not(target_arch = "wasm32"))]
    {
        molt_pipe_transport_close(handle_bits);
    }
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let mut map = pipe_transport_registry(_py).transports.lock().unwrap();
        map.remove(&handle);
    });
}

/// Connect a read pipe on the event loop.
///
/// `loop_handle`: event loop handle (u64 NaN-boxed int).
/// `fd_bits`: NaN-boxed integer file descriptor.
/// `callback_bits`: NaN-boxed callable (reader callback for data_received).
///
/// Creates a PipeTransport, registers the fd as a reader on the event loop,
/// and returns the pipe transport handle.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_connect_read_pipe(
    loop_handle: u64,
    fd_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        // Create the pipe transport (read mode).
        let pipe_handle = alloc_pipe_transport(_py, fd as i32, true);
        // Register the fd as a reader on the event loop.
        let Some(()) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return;
            }
            // Remove old reader if any.
            if let Some(old) = state.readers.remove(&fd) {
                dec_ref_bits(_py, old.callback_bits);
            }
            inc_ref_bits(_py, callback_bits);
            state.readers.insert(fd, IoCallbackEntry { callback_bits });
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::from_int(pipe_handle).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_connect_read_pipe(
    _loop_handle: u64,
    _fd_bits: u64,
    _callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "connect_read_pipe is not supported on WASM",
        )
    })
}

/// Connect a write pipe on the event loop.
///
/// `loop_handle`: event loop handle (u64 NaN-boxed int).
/// `fd_bits`: NaN-boxed integer file descriptor.
/// `callback_bits`: NaN-boxed callable (writer callback for write readiness).
///
/// Creates a PipeTransport, registers the fd as a writer on the event loop,
/// and returns the pipe transport handle.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_connect_write_pipe(
    loop_handle: u64,
    fd_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        // Create the pipe transport (write mode).
        let pipe_handle = alloc_pipe_transport(_py, fd as i32, false);
        // Register the fd as a writer on the event loop.
        let Some(()) = with_loop(_py, loop_handle, |state| {
            if state.is_closed() {
                return;
            }
            // Remove old writer if any.
            if let Some(old) = state.writers.remove(&fd) {
                dec_ref_bits(_py, old.callback_bits);
            }
            inc_ref_bits(_py, callback_bits);
            state.writers.insert(fd, IoCallbackEntry { callback_bits });
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::from_int(pipe_handle).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MoltObject, alloc_string, header_from_obj_ptr};

    fn ref_count(ptr: *mut u8) -> u32 {
        unsafe {
            (*header_from_obj_ptr(ptr))
                .ref_count
                .load(Ordering::Relaxed)
        }
    }

    #[test]
    fn event_loop_close_releases_all_callback_roots() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
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
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_connect_write_pipe(
    _loop_handle: u64,
    _fd_bits: u64,
    _callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "connect_write_pipe is not supported on WASM",
        )
    })
}
