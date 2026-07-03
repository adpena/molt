//! C-API diagnostic primitive: record the last C-API call that returned a
//! failure sentinel (NULL / -1) *without* a pending exception, and expose an
//! optional env-gated per-call trace.
//!
//! ## Why this exists
//!
//! CPython's C-API contract is: whenever an API returns an error sentinel
//! (NULL for object-returning calls, -1 for `int`-returning calls) it must set
//! a pending exception. A static C extension's module-exec slot (for example
//! numpy's `_multiarray_umath_exec`) relies on this: it calls dozens of C-API
//! functions in sequence, and on the first NULL/-1 it `return -1`s, trusting
//! that the exception is already set. If a Molt C-API implementation returns a
//! failure sentinel *without* setting the exception, the extension propagates a
//! bare `-1`, the import machinery reports the useless generic
//! "Py_mod_exec slot returned non-zero without setting an exception", and there
//! is no way to see *which* C-API call silently failed.
//!
//! This module is the reusable fix for that whole diagnostic gap. Molt C-API
//! functions that are about to return a failure sentinel record their name here
//! via [`record_silent_failure`] when no exception is pending. The static-init
//! failure path then surfaces the recorded name, turning an opaque failure into
//! a precise one. Each recorded name is a real Molt-owned bug to close (the
//! function must set an honest exception), so this primitive is both a
//! diagnostic and a to-do list.
//!
//! `MOLT_TRACE_CAPI=1` additionally streams every recorded silent failure to
//! stderr as it happens, so the *last* line before an exec returns `-1` names
//! the failing call directly.

use std::cell::RefCell;

thread_local! {
    /// Name of the most recent C-API call that returned a failure sentinel
    /// without a pending exception, plus optional detail (e.g. the attribute
    /// name). Cleared by [`take_last_silent_failure`].
    static LAST_SILENT_FAILURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn trace_enabled() -> bool {
    // Resolved per call rather than cached: the WASI host forwards the process
    // environment, and this path is cold (only hit on failure sentinels), so a
    // `getenv` here costs nothing measurable while staying robust to env setup
    // ordering across the split-runtime boundary.
    std::env::var_os("MOLT_TRACE_CAPI").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Record that `name` (with optional `detail`) returned a failure sentinel with
/// no pending exception. Overwrites any prior record for this thread so the
/// value read after an exec failure is the *last* silent failure. When
/// `MOLT_TRACE_CAPI` is set, also emits the record to stderr immediately.
pub fn record_silent_failure(name: &str, detail: Option<&str>) {
    let entry = match detail {
        Some(detail) if !detail.is_empty() => format!("{name}({detail})"),
        _ => name.to_string(),
    };
    if trace_enabled() {
        eprintln!("[MOLT_TRACE_CAPI] silent-failure {entry}");
    }
    LAST_SILENT_FAILURE.with(|slot| *slot.borrow_mut() = Some(entry));
}

/// Take and clear the last recorded silent failure for this thread.
pub fn take_last_silent_failure() -> Option<String> {
    LAST_SILENT_FAILURE.with(|slot| slot.borrow_mut().take())
}

/// Emit an env-gated trace line for a C-API call site. Only writes when
/// `MOLT_TRACE_CAPI` is set, so it is free on the normal path.
pub fn trace_call(name: &str, detail: Option<&str>) {
    if !trace_enabled() {
        return;
    }
    match detail {
        Some(detail) if !detail.is_empty() => eprintln!("[MOLT_TRACE_CAPI] call {name}({detail})"),
        _ => eprintln!("[MOLT_TRACE_CAPI] call {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_takes_last_silent_failure_with_detail() {
        // Start clean: a prior test on this thread must not leak state.
        let _ = take_last_silent_failure();
        record_silent_failure("PyObject_GetAttrString", Some("__array_finalize__"));
        assert_eq!(
            take_last_silent_failure().as_deref(),
            Some("PyObject_GetAttrString(__array_finalize__)")
        );
        // Taking clears the slot.
        assert_eq!(take_last_silent_failure(), None);
    }

    #[test]
    fn later_record_overwrites_so_the_last_failure_wins() {
        let _ = take_last_silent_failure();
        record_silent_failure("PyObject_Call", Some("module"));
        record_silent_failure("PyObject_GetAttr", None);
        assert_eq!(
            take_last_silent_failure().as_deref(),
            Some("PyObject_GetAttr")
        );
    }
}
