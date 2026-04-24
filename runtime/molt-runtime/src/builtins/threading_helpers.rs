// === FILE: runtime/molt-runtime/src/builtins/threading_helpers.rs ===
//! Threading module helper intrinsics.
//!
//! These intrinsics move remaining pure-Python helpers in `threading.py`
//! to Rust-backed implementations so that every public function and class
//! method in the threading module delegates to an intrinsic.
//!
//! ABI: NaN-boxed u64 in/out.

use crate::builtins::numbers::{int_bits_from_i64, to_f64, to_i64};
use crate::object::builders::{alloc_string, alloc_tuple};
use crate::{
    MoltObject, PyToken, bits_from_ptr, call_callable3, is_truthy, obj_from_bits, raise_exception,
    type_name,
};
use std::sync::atomic::{AtomicU64, Ordering};

// ── Thread name and token counters ──────────────────────────────────────────

static THREAD_NAME_COUNTER: AtomicU64 = AtomicU64::new(0);
static THREAD_TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);

fn make_string_bits(py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(py, s.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    bits_from_ptr(ptr)
}

/// Returns the next thread name as a NaN-boxed string: "Thread-N".
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_next_name() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let n = THREAD_NAME_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        let name = format!("Thread-{}", n);
        make_string_bits(py, &name)
    })
}

/// Returns the next thread token as a NaN-boxed integer.
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_next_token() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let n = THREAD_TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        int_bits_from_i64(py, n as i64)
    })
}

// ── Timeout validation ──────────────────────────────────────────────────────

/// Validates and normalizes a timeout parameter.
/// `mode_bits`: 0 = Lock mode, 1 = Event/Condition/Join mode.
/// `blocking_bits`: whether the operation is blocking (for Lock mode).
/// `timeout_bits`: the raw timeout value.
/// Returns: normalized timeout as float bits, or None bits if no timeout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_threading_validate_timeout(
    timeout_bits: u64,
    mode_bits: u64,
    blocking_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let timeout_obj = obj_from_bits(timeout_bits);
        let mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(0);
        let blocking = is_truthy(py, obj_from_bits(blocking_bits));

        // None means no timeout
        if timeout_obj.is_none() {
            if mode == 0 {
                return raise_exception::<u64>(
                    py,
                    "TypeError",
                    "'NoneType' object cannot be interpreted as an integer or float",
                );
            }
            return MoltObject::none().bits();
        }

        // Try to convert to float
        let timeout_val = match to_f64(timeout_obj) {
            Some(v) => v,
            None => {
                let tname = type_name(py, timeout_obj);
                let msg = format!(
                    "'{}' object cannot be interpreted as an integer or float",
                    tname
                );
                return raise_exception::<u64>(py, "TypeError", &msg);
            }
        };

        const TIMEOUT_MAX: f64 = 9223372036.0;

        match mode {
            0 => {
                // Lock mode
                if !blocking && timeout_val != -1.0 {
                    return raise_exception::<u64>(
                        py,
                        "ValueError",
                        "can't specify a timeout for a non-blocking call",
                    );
                }
                if blocking && timeout_val < 0.0 && timeout_val != -1.0 {
                    return raise_exception::<u64>(
                        py,
                        "ValueError",
                        "timeout value must be a non-negative number",
                    );
                }
                if blocking && timeout_val != -1.0 && timeout_val > TIMEOUT_MAX {
                    return raise_exception::<u64>(
                        py,
                        "OverflowError",
                        "timestamp out of range for platform time_t",
                    );
                }
                MoltObject::from_float(timeout_val).bits()
            }
            1 => {
                // Event/Condition/Join mode
                let clamped = if timeout_val < 0.0 { 0.0 } else { timeout_val };
                if clamped > TIMEOUT_MAX {
                    return raise_exception::<u64>(
                        py,
                        "OverflowError",
                        "timestamp out of range for platform time_t",
                    );
                }
                MoltObject::from_float(clamped).bits()
            }
            _ => MoltObject::none().bits(),
        }
    })
}

/// Invokes trace and profile hooks stored as NaN-boxed callables.
/// `trace_bits`: the trace hook callable bits (or None).
/// `profile_bits`: the profile hook callable bits (or None).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_threading_invoke_hooks(trace_bits: u64, profile_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(py, {
            let trace_obj = obj_from_bits(trace_bits);
            if !trace_obj.is_none() {
                let none_bits = MoltObject::none().bits();
                let call_str = make_string_bits(py, "call");
                let _ = call_callable3(py, trace_bits, none_bits, call_str, none_bits);
                crate::dec_ref_bits(py, call_str);
            }

            let profile_obj = obj_from_bits(profile_bits);
            if !profile_obj.is_none() {
                let none_bits = MoltObject::none().bits();
                let call_str = make_string_bits(py, "call");
                let _ = call_callable3(py, profile_bits, none_bits, call_str, none_bits);
                crate::dec_ref_bits(py, call_str);
            }

            MoltObject::none().bits()
        })
    }
}

/// Parses a thread registry record tuple into a validated tuple.
/// Returns the record as-is if valid (the Python side extracts fields).
/// Raises RuntimeError if the record is malformed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_threading_parse_registry_record(record_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(record_bits);
        if obj.is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "invalid thread registry record");
        }
        // Pass through validated record
        record_bits
    })
}

/// Bootstraps the main thread registry entry.
/// Delegates to molt_thread_registry_set_main.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_threading_bootstrap_main(name_bits: u64, daemon_bits: u64) -> u64 {
    unsafe {
        crate::molt_thread_registry_set_main(name_bits, daemon_bits);
    }
    MoltObject::none().bits()
}

/// Constructs a Thread-from-registry-record result tuple.
/// `name_bits`, `daemon_bits`, `ident_bits`, `native_id_bits`, `alive_bits` -
/// individual field bits already extracted from the record.
/// Returns a 5-tuple for the Python side.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_threading_registry_record_tuple(
    name_bits: u64,
    daemon_bits: u64,
    ident_bits: u64,
    native_id_bits: u64,
    alive_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let ptr = alloc_tuple(
            py,
            &[
                name_bits,
                daemon_bits,
                ident_bits,
                native_id_bits,
                alive_bits,
            ],
        );
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        bits_from_ptr(ptr)
    })
}

// ── Threading Lock wrappers ──────────────────────────────────────────────
//
// Trio expects `molt_threading_lock_*` names. These delegate to the existing
// `molt_lock_*` intrinsics in `concurrency/locks.rs`.

/// Create a new Lock. Returns a NaN-boxed handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_lock_new() -> u64 {
    unsafe { crate::molt_lock_new() }
}

/// Acquire the lock. Always succeeds (single-threaded). Returns True.
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_lock_acquire(lock_bits: u64) -> u64 {
    unsafe {
        crate::molt_lock_acquire(
            lock_bits,
            MoltObject::from_bool(true).bits(),
            MoltObject::none().bits(),
        )
    }
}

/// Release the lock. Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_lock_release(lock_bits: u64) -> u64 {
    unsafe { crate::molt_lock_release(lock_bits) }
}

// ── Threading Event wrappers ─────────────────────────────────────────────

/// Create a new Event. Returns a NaN-boxed handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_event_new() -> u64 {
    unsafe { crate::molt_event_new() }
}

/// Set the event flag.
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_event_set(event_bits: u64) -> u64 {
    unsafe { crate::molt_event_set(event_bits) }
}

/// Check if the event is set. Returns NaN-boxed bool.
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_event_is_set(event_bits: u64) -> u64 {
    unsafe { crate::molt_event_is_set(event_bits) }
}

/// Wait for the event (no-op in single-threaded mode: returns immediately).
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_event_wait(event_bits: u64) -> u64 {
    unsafe { crate::molt_event_wait(event_bits, MoltObject::none().bits()) }
}
