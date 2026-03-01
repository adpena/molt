// === FILE: runtime/molt-runtime/src/builtins/asyncio_core.rs ===
//
// Rust intrinsics for asyncio Future state machine and synchronization primitives
// (Event, Lock, Semaphore).
//
// Handle model: global LazyLock<Mutex<HashMap<i64, State>>> keyed by an
// atomically-issued handle ID, returned to Python as a NaN-boxed integer.
// Uses a global registry (not thread-local) so handles are visible across all
// threads — critical for asyncio primitives used cross-thread with
// concurrent.futures or multi-threaded event loops. The GIL serializes all
// Python-level access, so the Mutex is always uncontended.
//
// All stored u64 bits that may point to heap objects are inc_ref'd on store
// and dec_ref'd on removal/drop to maintain correct refcounts.
//
// WASM compatibility: ALL intrinsics in this module are pure state machines
// with no I/O, no file descriptors, no platform-specific syscalls, and no
// std::time usage. They compile and run correctly on all targets including
// wasm32-wasi and wasm32-unknown-unknown — no `#[cfg]` gating required.

use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

// ─── Handle counters ─────────────────────────────────────────────────────────

static NEXT_FUTURE_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_EVENT_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_LOCK_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_SEMAPHORE_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_future_handle() -> i64 {
    NEXT_FUTURE_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_event_handle() -> i64 {
    NEXT_EVENT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_lock_handle() -> i64 {
    NEXT_LOCK_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_semaphore_handle() -> i64 {
    NEXT_SEMAPHORE_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ─── Future state ────────────────────────────────────────────────────────────

struct FutureState {
    /// Result bits (None if not set). Heap objects are inc_ref'd.
    result_bits: u64,
    /// Exception bits (None if not set). Heap objects are inc_ref'd.
    exception_bits: u64,
    /// Whether this future has completed (result, exception, or cancelled).
    done: bool,
    /// Whether this future was cancelled.
    cancelled: bool,
    /// Cancel message bits (None if no message). Heap objects are inc_ref'd.
    cancel_msg_bits: u64,
    /// Done-callback bits. Each entry is inc_ref'd when stored.
    callbacks: Vec<u64>,
}

impl FutureState {
    fn new() -> Self {
        Self {
            result_bits: MoltObject::none().bits(),
            exception_bits: MoltObject::none().bits(),
            done: false,
            cancelled: false,
            cancel_msg_bits: MoltObject::none().bits(),
            callbacks: Vec::new(),
        }
    }
}

static FUTURES: LazyLock<Mutex<HashMap<i64, FutureState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── Event state ─────────────────────────────────────────────────────────────

struct EventState {
    flag: bool,
    /// Waiter bits (inc_ref'd on store).
    waiters: Vec<u64>,
}

impl EventState {
    fn new() -> Self {
        Self {
            flag: false,
            waiters: Vec::new(),
        }
    }
}

static EVENTS: LazyLock<Mutex<HashMap<i64, EventState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── Lock state ──────────────────────────────────────────────────────────────

struct LockState {
    locked: bool,
    /// Waiter bits (inc_ref'd on store).
    waiters: Vec<u64>,
}

impl LockState {
    fn new() -> Self {
        Self {
            locked: false,
            waiters: Vec::new(),
        }
    }
}

static LOCKS: LazyLock<Mutex<HashMap<i64, LockState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── Semaphore state ─────────────────────────────────────────────────────────

struct SemaphoreState {
    value: i64,
    /// Waiter bits (inc_ref'd on store).
    waiters: Vec<u64>,
}

impl SemaphoreState {
    fn new(initial_value: i64) -> Self {
        Self {
            value: initial_value,
            waiters: Vec::new(),
        }
    }
}

static SEMAPHORES: LazyLock<Mutex<HashMap<i64, SemaphoreState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── Helper: extract handle i64 from NaN-boxed bits ─────────────────────────

#[inline]
fn handle_from_bits(bits: u64) -> i64 {
    to_i64(obj_from_bits(bits)).unwrap_or(-1)
}

// ─── None sentinel constant ─────────────────────────────────────────────────

#[inline]
fn none_bits() -> u64 {
    MoltObject::none().bits()
}

// ─────────────────────────────────────────────────────────────────────────────
// Future intrinsics
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new future. Returns a handle as NaN-boxed integer bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = next_future_handle();
        FUTURES.lock().unwrap().insert(handle, FutureState::new());
        MoltObject::from_int(handle).bits()
    })
}

/// Get the result of a future. Raises InvalidStateError if not done.
/// If cancelled, raises CancelledError. If exception is set, raises it.
/// Otherwise returns the result bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_result(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let map = FUTURES.lock().unwrap();
        let Some(state) = map.get(&handle) else {
            return raise_exception::<u64>(_py, "InvalidStateError", "Future not found");
        };

        if !state.done {
            return raise_exception::<u64>(_py, "InvalidStateError", "Result is not ready");
        }

        if state.cancelled {
            // If an exception was stored (CancelledError with message), raise it.
            if !obj_from_bits(state.exception_bits).is_none() {
                // Return the exception bits for the Python layer to re-raise.
                let exc_bits = state.exception_bits;
                inc_ref_bits(_py, exc_bits);
                // Signal that the caller should raise CancelledError.
                return raise_exception::<u64>(_py, "CancelledError", "");
            }
            return raise_exception::<u64>(_py, "CancelledError", "");
        }

        if !obj_from_bits(state.exception_bits).is_none() {
            // The Python shim will raise the stored exception.
            // We return a sentinel to indicate exception state.
            return raise_exception::<u64>(_py, "FutureException", "exception is set");
        }

        let result = state.result_bits;
        inc_ref_bits(_py, result);
        result
    })
}

/// Get the exception of a future. Raises InvalidStateError if not done.
/// If cancelled, raises CancelledError. Otherwise returns exception bits
/// (may be None if no exception was set).
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_exception(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let map = FUTURES.lock().unwrap();
        let Some(state) = map.get(&handle) else {
            return raise_exception::<u64>(_py, "InvalidStateError", "Future not found");
        };

        if !state.done {
            return raise_exception::<u64>(_py, "InvalidStateError", "Result is not ready");
        }

        if state.cancelled {
            if !obj_from_bits(state.exception_bits).is_none() {
                return raise_exception::<u64>(_py, "CancelledError", "");
            }
            return raise_exception::<u64>(_py, "CancelledError", "");
        }

        let exc = state.exception_bits;
        inc_ref_bits(_py, exc);
        exc
    })
}

/// Atomic: set result + mark done + return callbacks count as int bits.
/// Raises InvalidStateError if already done.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_set_result_fast(handle_bits: u64, result_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = FUTURES.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return raise_exception::<u64>(_py, "InvalidStateError", "Future not found");
        };

        if state.done {
            return raise_exception::<u64>(_py, "InvalidStateError", "Result is already set");
        }

        // Store the result, inc_ref for heap objects.
        inc_ref_bits(_py, result_bits);
        state.result_bits = result_bits;
        state.done = true;

        let cb_count = state.callbacks.len() as i64;
        MoltObject::from_int(cb_count).bits()
    })
}

/// Atomic: set exception + mark done + return callbacks count as int bits.
/// Raises InvalidStateError if already done. If the exception is a
/// CancelledError, also marks the future as cancelled.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_set_exception_fast(handle_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = FUTURES.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return raise_exception::<u64>(_py, "InvalidStateError", "Future not found");
        };

        if state.done {
            return raise_exception::<u64>(_py, "InvalidStateError", "Result is already set");
        }

        // Store the exception, inc_ref for heap objects.
        inc_ref_bits(_py, exc_bits);
        state.exception_bits = exc_bits;
        state.done = true;

        // Note: The Python layer is responsible for checking if the exception
        // is a CancelledError and calling cancel_fast instead if so. This
        // intrinsic handles the general exception case.

        let cb_count = state.callbacks.len() as i64;
        MoltObject::from_int(cb_count).bits()
    })
}

/// Cancel a future with an optional message. Returns bool (True if cancelled,
/// False if already done).
///
/// msg_bits: cancel message (may be None for no message).
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_cancel_fast(handle_bits: u64, msg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = FUTURES.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return MoltObject::from_bool(false).bits();
        };

        if state.done {
            return MoltObject::from_bool(false).bits();
        }

        state.cancelled = true;
        state.done = true;

        // Store cancel message if provided.
        let msg_obj = obj_from_bits(msg_bits);
        if !msg_obj.is_none() {
            inc_ref_bits(_py, msg_bits);
            state.cancel_msg_bits = msg_bits;
        }

        // Dec-ref any previously stored exception/result (should be None but
        // be defensive).
        let old_exc = state.exception_bits;
        let old_res = state.result_bits;
        state.exception_bits = none_bits();
        state.result_bits = none_bits();
        dec_ref_bits(_py, old_exc);
        dec_ref_bits(_py, old_res);

        let _cb_count = state.callbacks.len() as i64;
        // Return True to signal cancellation succeeded; the Python layer
        // will call _invoke_callbacks itself.
        MoltObject::from_bool(true).bits()
    })
}

/// Check if a future is done (result set, exception set, or cancelled).
/// Returns bool bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_done(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let map = FUTURES.lock().unwrap();
        let Some(state) = map.get(&handle) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(state.done).bits()
    })
}

/// Check if a future was cancelled. Returns bool bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_cancelled(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let map = FUTURES.lock().unwrap();
        let Some(state) = map.get(&handle) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(state.cancelled).bits()
    })
}

/// Add a done-callback to a future. If the future is already done, returns
/// True (was_done) so the Python layer can invoke the callback immediately.
/// Otherwise stores the callback and returns False.
///
/// callback_bits: the callable bits to invoke when the future completes.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_add_done_callback_fast(
    handle_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = FUTURES.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return MoltObject::from_bool(true).bits();
        };

        if state.done {
            // Future already done — caller should invoke callback immediately.
            return MoltObject::from_bool(true).bits();
        }

        // Store callback with inc_ref.
        inc_ref_bits(_py, callback_bits);
        state.callbacks.push(callback_bits);
        MoltObject::from_bool(false).bits()
    })
}

/// Drop a future handle. Dec-refs all stored bits and removes from registry.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_future_drop(handle_bits: u64) {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let removed = FUTURES.lock().unwrap().remove(&handle);
        if let Some(state) = removed {
            // Dec-ref stored heap objects.
            dec_ref_bits(_py, state.result_bits);
            dec_ref_bits(_py, state.exception_bits);
            dec_ref_bits(_py, state.cancel_msg_bits);
            for cb in &state.callbacks {
                dec_ref_bits(_py, *cb);
            }
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Event intrinsics
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new asyncio.Event. Returns handle as NaN-boxed integer bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = next_event_handle();
        EVENTS.lock().unwrap().insert(handle, EventState::new());
        MoltObject::from_int(handle).bits()
    })
}

/// Check if an event's internal flag is set. Returns bool bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_is_set(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let map = EVENTS.lock().unwrap();
        let Some(state) = map.get(&handle) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(state.flag).bits()
    })
}

/// Set the event's internal flag. Returns the waiter count as int bits
/// so the Python layer can notify all waiters. Idempotent: if already set,
/// returns 0.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_set_fast(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = EVENTS.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return MoltObject::from_int(0).bits();
        };

        if state.flag {
            // Already set — no waiters to notify.
            return MoltObject::from_int(0).bits();
        }

        state.flag = true;

        // Drain waiters — the Python layer will set_result(True) on each.
        let waiter_count = state.waiters.len() as i64;

        // Dec-ref the waiters as we remove them from our storage.
        // The Python layer holds its own references via the waiters list.
        for w in state.waiters.drain(..) {
            dec_ref_bits(_py, w);
        }

        MoltObject::from_int(waiter_count).bits()
    })
}

/// Clear the event's internal flag. Returns None bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_clear(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = EVENTS.lock().unwrap();
        if let Some(state) = map.get_mut(&handle) {
            state.flag = false;
        }
        none_bits()
    })
}

/// Drop an event handle. Dec-refs all stored waiter bits and removes from registry.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_drop(handle_bits: u64) {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let removed = EVENTS.lock().unwrap().remove(&handle);
        if let Some(state) = removed {
            for w in &state.waiters {
                dec_ref_bits(_py, *w);
            }
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Lock intrinsics
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new asyncio.Lock. Returns handle as NaN-boxed integer bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_lock_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = next_lock_handle();
        LOCKS.lock().unwrap().insert(handle, LockState::new());
        MoltObject::from_int(handle).bits()
    })
}

/// Check if a lock is currently acquired. Returns bool bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_lock_locked(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let map = LOCKS.lock().unwrap();
        let Some(state) = map.get(&handle) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(state.locked).bits()
    })
}

/// Try to acquire the lock immediately. Returns True if acquired (lock was
/// unlocked), False if the lock is already held (caller must await).
///
/// This is the fast path — the Python layer calls this first, and only if
/// it returns False does it create a Future waiter and park.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_lock_acquire_fast(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = LOCKS.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return MoltObject::from_bool(false).bits();
        };

        if !state.locked {
            state.locked = true;
            MoltObject::from_bool(true).bits()
        } else {
            MoltObject::from_bool(false).bits()
        }
    })
}

/// Release the lock. If there are waiters, returns the waiter count (>0)
/// so the Python layer can notify the next waiter. If no waiters, unlocks
/// and returns 0. Raises RuntimeError if the lock is not held.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_lock_release_fast(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = LOCKS.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return raise_exception::<u64>(_py, "RuntimeError", "Lock not found");
        };

        if !state.locked {
            return raise_exception::<u64>(_py, "RuntimeError", "Lock is not acquired");
        }

        if !state.waiters.is_empty() {
            // There are waiters — the Python layer should notify the first one.
            // We don't unlock here; the next waiter's acquire will keep it locked.
            let waiter_count = state.waiters.len() as i64;

            // Pop the first waiter and dec_ref it (we held a ref).
            let first_waiter = state.waiters.remove(0);
            dec_ref_bits(_py, first_waiter);

            // Return the remaining waiter count + 1 (including the one we popped)
            // to signal that notification is needed.
            MoltObject::from_int(waiter_count).bits()
        } else {
            // No waiters — just unlock.
            state.locked = false;
            MoltObject::from_int(0).bits()
        }
    })
}

/// Drop a lock handle. Dec-refs all stored waiter bits and removes from registry.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_lock_drop(handle_bits: u64) {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let removed = LOCKS.lock().unwrap().remove(&handle);
        if let Some(state) = removed {
            for w in &state.waiters {
                dec_ref_bits(_py, *w);
            }
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Semaphore intrinsics
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new asyncio.Semaphore with the given initial value.
/// value_bits: NaN-boxed integer for the initial counter value.
/// Returns handle as NaN-boxed integer bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_semaphore_new(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let initial_value = to_i64(obj_from_bits(value_bits)).unwrap_or(1);
        if initial_value < 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "Semaphore initial value must be >= 0",
            );
        }
        let handle = next_semaphore_handle();
        SEMAPHORES
            .lock()
            .unwrap()
            .insert(handle, SemaphoreState::new(initial_value));
        MoltObject::from_int(handle).bits()
    })
}

/// Try to acquire the semaphore immediately. Returns True if acquired
/// (counter was > 0 and has been decremented), False if the counter is 0
/// (caller must await).
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_semaphore_acquire_fast(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let mut map = SEMAPHORES.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return MoltObject::from_bool(false).bits();
        };

        if state.value > 0 {
            state.value -= 1;
            MoltObject::from_bool(true).bits()
        } else {
            MoltObject::from_bool(false).bits()
        }
    })
}

/// Release the semaphore. If there are waiters, pops the first waiter and
/// returns the total waiter count (including the popped one) so the Python
/// layer can notify it. If no waiters, increments the counter.
///
/// max_value_bits: for BoundedSemaphore, the initial value cap. Pass None
/// or -1 for unbounded Semaphore. If the counter would exceed max_value,
/// raises ValueError.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_semaphore_release_fast(
    handle_bits: u64,
    max_value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let max_value_obj = obj_from_bits(max_value_bits);
        let max_value: Option<i64> = if max_value_obj.is_none() {
            None
        } else {
            to_i64(max_value_obj).and_then(|v| if v < 0 { None } else { Some(v) })
        };

        let mut map = SEMAPHORES.lock().unwrap();
        let Some(state) = map.get_mut(&handle) else {
            return raise_exception::<u64>(_py, "RuntimeError", "Semaphore not found");
        };

        if !state.waiters.is_empty() {
            // Waiters present — pop the first one and signal the Python layer.
            let waiter_count = state.waiters.len() as i64;
            let first_waiter = state.waiters.remove(0);
            dec_ref_bits(_py, first_waiter);
            MoltObject::from_int(waiter_count).bits()
        } else {
            // No waiters — increment counter.
            // BoundedSemaphore check: if max_value is set, verify we don't exceed it.
            if max_value.is_some_and(|max| state.value >= max) {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "BoundedSemaphore released too many times",
                );
            }
            state.value += 1;
            MoltObject::from_int(0).bits()
        }
    })
}

/// Query the current counter value of a semaphore.
/// Returns the value as NaN-boxed integer bits, or -1 if the handle is not found.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_semaphore_value(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let map = SEMAPHORES.lock().unwrap();
        let value = map.get(&handle).map_or(0, |s| s.value);
        MoltObject::from_int(value).bits()
    })
}

/// Drop a semaphore handle. Dec-refs all stored waiter bits and removes from registry.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_semaphore_drop(handle_bits: u64) {
    crate::with_gil_entry!(_py, {
        let handle = handle_from_bits(handle_bits);
        let removed = SEMAPHORES.lock().unwrap().remove(&handle);
        if let Some(state) = removed {
            for w in &state.waiters {
                dec_ref_bits(_py, *w);
            }
        }
    })
}
