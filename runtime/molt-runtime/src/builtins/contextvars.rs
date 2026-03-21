// === FILE: runtime/molt-runtime/src/builtins/contextvars.rs ===
//! Intrinsic implementations for the `contextvars` module.
//!
//! Provides task-local context variables needed by trio's structured concurrency
//! runtime. Each `ContextVar` is identified by a unique integer handle.
//! The context is a stack of frames (`HashMap<i64, u64>`), where each frame
//! maps variable handles to NaN-boxed Python object bits.
//!
//! A `Token` records the previous value so `reset()` can restore it.
//! `copy_context()` snapshots the top frame for use in spawned tasks.
//!
//! Since molt targets single-threaded WASM, we use `thread_local!` with no
//! mutex overhead. All values are NaN-boxed u64.
//!
//! Refcount protocol:
//! - Values stored into a context frame are `inc_ref`'d; values removed are
//!   `dec_ref`'d.
//! - Tokens hold a reference to the old value (if any).

use crate::builtins::numbers::int_bits_from_i64;
use crate::{MoltObject, dec_ref_bits, inc_ref_bits, obj_from_bits, raise_exception, to_i64};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

// ─── Handle counters ──────────────────────────────────────────────────────

static NEXT_VAR_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_TOKEN_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_CONTEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

// ─── Context var default values ───────────────────────────────────────────
//
// Maps var_handle -> default_bits.  Stored globally (not per-context) because
// the default is set once at creation time and never changes.

thread_local! {
    static VAR_DEFAULTS: RefCell<HashMap<i64, u64>> = RefCell::new(HashMap::new());
}

// ─── Context frames ──────────────────────────────────────────────────────
//
// Stack of context frames. The top frame is the "current" context.
// Each frame maps var_handle -> value_bits.

thread_local! {
    static CONTEXT_FRAMES: RefCell<Vec<HashMap<i64, u64>>> = RefCell::new(vec![HashMap::new()]);
}

// ─── Token registry ──────────────────────────────────────────────────────
//
// token_handle -> (var_handle, old_value_bits, used_flag)
// old_value_bits is the value *before* the set() that created this token.
// A sentinel of `MISSING` means the variable had no value before.

const MISSING: u64 = u64::MAX; // sentinel: no previous value

thread_local! {
    static TOKEN_REGISTRY: RefCell<HashMap<i64, (i64, u64, bool)>> =
        RefCell::new(HashMap::new());
}

// ─── Saved context snapshots ─────────────────────────────────────────────
//
// context_handle -> HashMap<i64, u64>   (a frozen copy of a frame)

thread_local! {
    static CONTEXT_REGISTRY: RefCell<HashMap<i64, HashMap<i64, u64>>> =
        RefCell::new(HashMap::new());
}

// ─── Intrinsics ──────────────────────────────────────────────────────────

/// Create a new ContextVar.
/// `name_bits`: NaN-boxed string (ignored for storage, kept for Python repr).
/// `default_bits`: NaN-boxed default value, or None if no default.
/// Returns: NaN-boxed integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_new_var(name_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(py, {
        let handle = NEXT_VAR_HANDLE.fetch_add(1, Ordering::Relaxed);
        // Store the default (inc_ref if not None).
        let default_obj = obj_from_bits(default_bits);
        if !default_obj.is_none() {
            inc_ref_bits(py, default_bits);
        }
        VAR_DEFAULTS.with(|d| {
            d.borrow_mut().insert(handle, default_bits);
        });
        // name_bits is unused internally but we consume the argument.
        let _ = name_bits;
        int_bits_from_i64(py, handle)
    })
}

/// Get the current value of a ContextVar.
/// `var_bits`: NaN-boxed integer handle (from new_var).
/// Returns: the current value, or the default, or raises LookupError.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_get(var_bits: u64) -> u64 {
    crate::with_gil_entry!(py, {
        let Some(handle) = to_i64(obj_from_bits(var_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };

        // Look up in the top context frame.
        let found = CONTEXT_FRAMES.with(|frames| {
            let frames = frames.borrow();
            if let Some(top) = frames.last() {
                top.get(&handle).copied()
            } else {
                None
            }
        });

        if let Some(bits) = found {
            inc_ref_bits(py, bits);
            return bits;
        }

        // Fall back to default.
        let default = VAR_DEFAULTS.with(|d| d.borrow().get(&handle).copied());
        match default {
            Some(bits) if !obj_from_bits(bits).is_none() => {
                inc_ref_bits(py, bits);
                bits
            }
            _ => raise_exception::<u64>(py, "LookupError", "ContextVar has no value and no default"),
        }
    })
}

/// Set the value of a ContextVar in the current context.
/// Returns: NaN-boxed integer token handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_set(var_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(py, {
        let Some(handle) = to_i64(obj_from_bits(var_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };

        // Capture old value for the token.
        let old_bits = CONTEXT_FRAMES.with(|frames| {
            let frames = frames.borrow();
            frames.last().and_then(|top| top.get(&handle).copied())
        });

        let token_old = old_bits.unwrap_or(MISSING);

        // Inc-ref old value in token (if present).
        if token_old != MISSING {
            inc_ref_bits(py, token_old);
        }

        // Store new value in top frame.
        inc_ref_bits(py, value_bits);
        let evicted = CONTEXT_FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            let top = frames.last_mut().expect("context frame stack empty");
            top.insert(handle, value_bits)
        });

        // Dec-ref the evicted value (if any).
        if let Some(old) = evicted {
            dec_ref_bits(py, old);
        }

        // Allocate a token.
        let token_handle = NEXT_TOKEN_HANDLE.fetch_add(1, Ordering::Relaxed);
        TOKEN_REGISTRY.with(|r| {
            r.borrow_mut()
                .insert(token_handle, (handle, token_old, false));
        });

        int_bits_from_i64(py, token_handle)
    })
}

/// Reset a ContextVar to the value it had before the corresponding set().
/// `var_bits`: NaN-boxed integer var handle.
/// `token_bits`: NaN-boxed integer token handle.
/// Returns: None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_reset(var_bits: u64, token_bits: u64) -> u64 {
    crate::with_gil_entry!(py, {
        let Some(caller_var) = to_i64(obj_from_bits(var_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };
        let Some(token_handle) = to_i64(obj_from_bits(token_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };

        let entry = TOKEN_REGISTRY.with(|r| r.borrow().get(&token_handle).copied());

        let Some((var_handle, old_bits, used)) = entry else {
            return raise_exception::<u64>(py, "ValueError", "Token is invalid");
        };

        if caller_var != var_handle {
            return raise_exception::<u64>(py, "ValueError",
                "Token was created by a different ContextVar");
        }

        if used {
            return raise_exception::<u64>(py, "RuntimeError", "Token has already been used");
        }

        // Mark token as used.
        TOKEN_REGISTRY.with(|r| {
            if let Some(entry) = r.borrow_mut().get_mut(&token_handle) {
                entry.2 = true;
            }
        });

        // Restore old value in top frame.
        CONTEXT_FRAMES.with(|frames| {
            let mut frames = frames.borrow_mut();
            let top = frames.last_mut().expect("context frame stack empty");

            // Dec-ref current value.
            if let Some(current) = top.remove(&var_handle) {
                dec_ref_bits(py, current);
            }

            // Restore old value.
            if old_bits != MISSING {
                // The token held a ref; transfer it to the frame.
                top.insert(var_handle, old_bits);
            }
        });

        MoltObject::none().bits()
    })
}

/// Copy the current context into a new snapshot.
/// Returns: NaN-boxed integer context handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_copy_context() -> u64 {
    crate::with_gil_entry!(py, {
        let ctx_handle = NEXT_CONTEXT_HANDLE.fetch_add(1, Ordering::Relaxed);

        let snapshot = CONTEXT_FRAMES.with(|frames| {
            let frames = frames.borrow();
            frames.last().cloned().unwrap_or_default()
        });

        // Inc-ref all values in the snapshot.
        for &bits in snapshot.values() {
            inc_ref_bits(py, bits);
        }

        CONTEXT_REGISTRY.with(|r| {
            r.borrow_mut().insert(ctx_handle, snapshot);
        });

        int_bits_from_i64(py, ctx_handle)
    })
}
