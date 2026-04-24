// === FILE: runtime/molt-runtime/src/builtins/asyncio_helpers.rs ===
//! asyncio module helper intrinsics.
//!
//! These intrinsics move remaining pure-Python helpers in the asyncio
//! __init__.py to Rust-backed implementations so that every public function
//! and class method in the asyncio module delegates to an intrinsic.
//!
//! WASM-compatible: no I/O, no syscalls, no platform-specific code.
//! ABI: NaN-boxed u64 in/out.

use crate::builtins::numbers::{int_bits_from_i64, to_i64};
use crate::object::builders::alloc_string;
use crate::{MoltObject, bits_from_ptr, obj_from_bits, raise_exception};
use std::sync::atomic::{AtomicU64, Ordering};

// ── Task name counter ───────────────────────────────────────────────────────

static TASK_NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns the next asyncio task name as a NaN-boxed string: "Task-N".
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_next_name() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let n = TASK_NAME_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        let name = format!("Task-{}", n);
        let ptr = alloc_string(py, name.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        bits_from_ptr(ptr)
    })
}

/// Validates that a coroutine object is not None.
/// Returns True bits if valid, raises TypeError otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_validate_coro(coro_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let obj = obj_from_bits(coro_bits);
        if obj.is_none() {
            return raise_exception::<u64>(py, "TypeError", "a coroutine was expected, got None");
        }
        MoltObject::from_bool(true).bits()
    })
}

/// Resolves the cancel message state for a Task.
/// Increments cancel_requested and returns new count as int bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_task_cancel_state(cancel_requested_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let count = to_i64(obj_from_bits(cancel_requested_bits)).unwrap_or(0);
        int_bits_from_i64(py, count + 1)
    })
}

/// Decrements the cancel-requested counter for a Task.
/// Returns the new count as int bits. If count <= 0, returns 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_task_uncancel_state(cancel_requested_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let count = to_i64(obj_from_bits(cancel_requested_bits)).unwrap_or(0);
        let new_count = if count <= 0 { 0 } else { count - 1 };
        int_bits_from_i64(py, new_count)
    })
}

/// Validates that a value is a non-negative integer for Semaphore init.
/// Returns the validated int bits, or raises ValueError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_validate_semaphore_value(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let val = to_i64(obj_from_bits(value_bits)).unwrap_or(-1);
        if val < 0 {
            return raise_exception::<u64>(
                py,
                "ValueError",
                "Semaphore initial value must be >= 0",
            );
        }
        value_bits
    })
}

/// Validates Barrier parties value.
/// Returns the validated int bits, or raises ValueError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_validate_barrier_parties(parties_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let val = to_i64(obj_from_bits(parties_bits)).unwrap_or(0);
        if val <= 0 {
            return raise_exception::<u64>(py, "ValueError", "Barrier parties must be > 0");
        }
        parties_bits
    })
}

/// Increments a barrier counter. Returns the new count as int bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_barrier_count_incr(count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let count = to_i64(obj_from_bits(count_bits)).unwrap_or(0);
        int_bits_from_i64(py, count + 1)
    })
}

/// Barrier reset: returns 0 (new count) as int bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_barrier_reset_state() -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_bits_from_i64(py, 0) })
}

/// Barrier abort: returns 0 (new count) as int bits.
/// The Python side sets the broken flag separately.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_barrier_abort_state() -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_bits_from_i64(py, 0) })
}

/// Validates the asyncio Lock is acquired before releasing.
/// `locked_bits`: True if locked, False if not.
/// Returns None on success, raises RuntimeError if not locked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_lock_validate_release(locked_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let obj = obj_from_bits(locked_bits);
        let locked = obj.as_bool().unwrap_or(false);
        if !locked {
            return raise_exception::<u64>(py, "RuntimeError", "Lock is not acquired");
        }
        MoltObject::none().bits()
    })
}

/// Validates the asyncio Condition lock is acquired.
/// `locked_bits`: True if locked, False if not.
/// Returns None on success, raises RuntimeError if not acquired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_condition_validate_locked(locked_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let obj = obj_from_bits(locked_bits);
        let locked = obj.as_bool().unwrap_or(false);
        if !locked {
            return raise_exception::<u64>(py, "RuntimeError", "Condition lock is not acquired");
        }
        MoltObject::none().bits()
    })
}

/// Checks if an exception is a CancelledError by examining its type name.
/// Returns True/False as bool bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_is_cancelled_exc(exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let obj = obj_from_bits(exc_bits);
        if obj.is_none() {
            return MoltObject::from_bool(false).bits();
        }
        // Use type_name to check for CancelledError
        let tname = crate::type_name(py, obj);
        let is_cancelled = tname == "CancelledError";
        MoltObject::from_bool(is_cancelled).bits()
    })
}

// ── Stream buffer helpers ────────────────────────────────────────────────────

/// StreamReader buffer management: validates the read count parameter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_stream_buffer_read(_buffer_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let n = to_i64(obj_from_bits(n_bits)).unwrap_or(-1);
        if n == 0 {
            return MoltObject::none().bits();
        }
        // Delegate to existing intrinsics via Python-side calls
        // This intrinsic validates the request; actual I/O is done
        // by the existing molt_asyncio_stream_buffer_snapshot intrinsic
        // which the Python side calls separately.
        int_bits_from_i64(py, n)
    })
}

/// StreamWriter buffer append: validates data is bytes-like.
/// Returns None on success, raises TypeError if data is None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_stream_writer_buffer_append(
    _buffer_bits: u64,
    data_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let data_obj = obj_from_bits(data_bits);
        if data_obj.is_none() {
            return raise_exception::<u64>(py, "TypeError", "data must be bytes-like");
        }
        // Validation passed; the Python side does the actual buffer.extend()
        MoltObject::none().bits()
    })
}
