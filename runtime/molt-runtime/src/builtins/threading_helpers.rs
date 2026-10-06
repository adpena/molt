// === FILE: runtime/molt-runtime/src/builtins/threading_helpers.rs ===
//! Threading module helper intrinsics.
//!
//! These intrinsics move remaining pure-Python helpers in `threading.py`
//! to Rust-backed implementations so that every public function and class
//! method in the threading module delegates to an intrinsic.
//!
//! ABI: NaN-boxed u64 in/out.

use crate::builtins::numbers::int_bits_from_i64;
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

/// Semantic policy at the wait boundary, before platform timeout conversion.
#[derive(Clone, Copy)]
pub(crate) enum ThreadTimeoutPolicy {
    /// _thread.Lock/RLock convert first, then validate blocking and -1.
    Lock { blocking: bool },
    /// Condition (and pending Future) compare with zero before lock conversion.
    Condition,
    /// Thread.join clamps negative values with max(timeout, 0), then converts.
    Join,
}

// CPython Include/pythread.h: Windows reserves DWORD::MAX for INFINITE;
// POSIX timed locks use signed nanoseconds. These are guest target facts,
// not properties of the host running the compiler or Python frontend.
#[cfg(target_os = "windows")]
const THREAD_TIMEOUT_MAX_MICROS: i64 = (u32::MAX as i64 - 1) * 1_000;
#[cfg(not(target_os = "windows"))]
const THREAD_TIMEOUT_MAX_MICROS: i64 = i64::MAX / 1_000;

#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_timeout_max() -> u64 {
    MoltObject::from_float((THREAD_TIMEOUT_MAX_MICROS / 1_000_000) as f64).bits()
}

fn timeout_nanoseconds(py: &PyToken<'_>, bits: u64) -> Result<i64, u64> {
    use num_traits::ToPrimitive;
    let object = obj_from_bits(bits);
    let float = crate::as_float_extended(object);
    if let Some(seconds) = float {
        if seconds.is_nan() {
            return Err(raise_exception::<u64>(
                py,
                "ValueError",
                "Invalid value NaN (not a number)",
            ));
        }
        // Timeout rounding is away from zero, preserving positive sub-nanosecond
        // waits. Check the signed time range before any integer cast.
        let nanos = if seconds >= 0.0 {
            (seconds * 1_000_000_000.0).ceil()
        } else {
            (seconds * 1_000_000_000.0).floor()
        };
        if !nanos.is_finite() || nanos >= i64::MAX as f64 || nanos < i64::MIN as f64 {
            return Err(raise_exception::<u64>(
                py,
                "OverflowError",
                "timestamp out of range for platform time_t",
            ));
        }
        return Ok(nanos as i64);
    }
    // PyTime accepts actual floats or __index__, never the broader float()
    // protocol (which accepts strings and __float__-only objects).
    let message = format!(
        "'{}' object cannot be interpreted as an integer",
        type_name(py, object)
    );
    let Some(seconds) = crate::builtins::numbers::index_bigint_from_obj(py, bits, &message) else {
        return Err(MoltObject::none().bits());
    };
    match seconds
        .to_i64()
        .and_then(|value| value.checked_mul(1_000_000_000))
    {
        Some(value) => Ok(value),
        None => Err(raise_exception::<u64>(
            py,
            "OverflowError",
            "timestamp too large to convert to C _PyTime_t",
        )),
    }
}

fn positive_timeout_duration(py: &PyToken<'_>, nanos: i64) -> Result<std::time::Duration, u64> {
    debug_assert!(nanos >= 0);
    // The timed-lock API consumes microseconds rounded up. Validate this
    // exact projection, including the fractional interval above TIMEOUT_MAX.
    let micros = nanos / 1_000 + i64::from(nanos % 1_000 != 0);
    if micros > THREAD_TIMEOUT_MAX_MICROS {
        return Err(raise_exception::<u64>(
            py,
            "OverflowError",
            "timeout value is too large",
        ));
    }
    Ok(std::time::Duration::from_nanos(nanos as u64))
}

fn timeout_comparison(py: &PyToken<'_>, left: u64, right: u64) -> Result<bool, u64> {
    let compared = crate::molt_gt(left, right);
    if crate::exception_pending(py) {
        crate::dec_ref_bits(py, compared);
        return Err(MoltObject::none().bits());
    }
    let result = is_truthy(py, obj_from_bits(compared));
    crate::dec_ref_bits(py, compared);
    if crate::exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(result)
    }
}

pub(crate) fn parse_thread_timeout(
    py: &PyToken<'_>,
    bits: u64,
    policy: ThreadTimeoutPolicy,
) -> Result<Option<std::time::Duration>, u64> {
    let blocking = match policy {
        ThreadTimeoutPolicy::Lock { blocking } => blocking,
        ThreadTimeoutPolicy::Condition | ThreadTimeoutPolicy::Join => {
            if obj_from_bits(bits).is_none() {
                return Ok(None);
            }
            let zero = MoltObject::from_int(0).bits();
            let immediate = match policy {
                ThreadTimeoutPolicy::Condition => !timeout_comparison(py, bits, zero)?,
                ThreadTimeoutPolicy::Join => timeout_comparison(py, zero, bits)?,
                ThreadTimeoutPolicy::Lock { .. } => unreachable!(),
            };
            if immediate {
                return Ok(Some(std::time::Duration::ZERO));
            }
            true
        }
    };
    let nanos = timeout_nanoseconds(py, bits)?;
    if !blocking && nanos != -1_000_000_000 {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            "can't specify a timeout for a non-blocking call",
        ));
    }
    if nanos < 0 && nanos != -1_000_000_000 {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            "timeout value must be positive",
        ));
    }
    if nanos == -1_000_000_000 {
        return Ok(None);
    }
    positive_timeout_duration(py, nanos).map(Some)
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
                crate::call::discard_owned_call_result(
                    py,
                    call_callable3(py, trace_bits, none_bits, call_str, none_bits),
                );
                crate::dec_ref_bits(py, call_str);
            }

            let profile_obj = obj_from_bits(profile_bits);
            if !profile_obj.is_none() {
                let none_bits = MoltObject::none().bits();
                let call_str = make_string_bits(py, "call");
                crate::call::discard_owned_call_result(
                    py,
                    call_callable3(py, profile_bits, none_bits, call_str, none_bits),
                );
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

#[cfg(test)]
mod timeout_tests {
    use super::*;
    use std::time::Duration;

    fn assert_error(py: &PyToken<'_>, bits: u64, policy: ThreadTimeoutPolicy, name: &str) {
        assert!(parse_thread_timeout(py, bits, policy).is_err());
        let pending = crate::molt_exception_last_pending();
        let ptr = obj_from_bits(pending).as_ptr().expect("pending exception");
        let class = crate::builtins::exceptions::exception_type_bits_from_name(py, name);
        assert_eq!(unsafe { crate::object_class_bits(ptr) }, class);
        crate::clear_exception(py);
        crate::dec_ref_bits(py, pending);
    }

    #[test]
    fn lock_condition_and_join_keep_distinct_timeout_policies() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let lock = ThreadTimeoutPolicy::Lock { blocking: true };
            let condition = ThreadTimeoutPolicy::Condition;
            for value in [f64::NAN, f64::NEG_INFINITY, -2.0] {
                let bits = crate::float_result_bits(py, value);
                assert_eq!(
                    parse_thread_timeout(py, bits, condition).unwrap(),
                    Some(Duration::ZERO)
                );
                assert_error(
                    py,
                    bits,
                    lock,
                    if value.is_infinite() {
                        "OverflowError"
                    } else {
                        "ValueError"
                    },
                );
                crate::dec_ref_bits(py, bits);
            }
            let nan = crate::float_result_bits(py, f64::NAN);
            assert_error(py, nan, ThreadTimeoutPolicy::Join, "ValueError");
            crate::dec_ref_bits(py, nan);
            for value in [f64::INFINITY, 1e300] {
                let bits = crate::float_result_bits(py, value);
                assert_error(py, bits, condition, "OverflowError");
                assert_error(py, bits, lock, "OverflowError");
                crate::dec_ref_bits(py, bits);
            }
            let none = MoltObject::none().bits();
            assert_eq!(parse_thread_timeout(py, none, condition).unwrap(), None);
            assert_error(py, none, lock, "TypeError");
            let negative_one = MoltObject::from_int(-1).bits();
            assert_eq!(parse_thread_timeout(py, negative_one, lock).unwrap(), None);
            assert_eq!(
                parse_thread_timeout(py, negative_one, condition).unwrap(),
                Some(Duration::ZERO)
            );
            let tiny = MoltObject::from_float(1e-12).bits();
            assert_eq!(
                parse_thread_timeout(py, tiny, lock).unwrap(),
                Some(Duration::from_nanos(1))
            );
            assert_error(
                py,
                MoltObject::from_int(0).bits(),
                ThreadTimeoutPolicy::Lock { blocking: false },
                "ValueError",
            );
        });
    }

    #[test]
    fn platform_limit_and_integer_time_overflow_are_checked_before_duration_creation() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let limit_nanos = THREAD_TIMEOUT_MAX_MICROS * 1_000;
            assert_eq!(
                positive_timeout_duration(py, limit_nanos)
                    .unwrap()
                    .as_nanos(),
                limit_nanos as u128
            );
            assert!(positive_timeout_duration(py, limit_nanos + 1).is_err());
            crate::clear_exception(py);
            let over_seconds = MoltObject::from_int(i64::MAX / 1_000_000_000 + 1).bits();
            assert_error(
                py,
                over_seconds,
                ThreadTimeoutPolicy::Lock { blocking: true },
                "OverflowError",
            );
            let max_seconds = crate::to_f64(obj_from_bits(molt_thread_timeout_max())).unwrap();
            #[cfg(target_os = "windows")]
            assert_eq!(max_seconds, 4_294_967.0);
            #[cfg(not(target_os = "windows"))]
            assert_eq!(max_seconds, 9_223_372_036.0);
        });
    }
}
