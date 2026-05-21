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
//! State is owned by `RuntimeState`, with per-thread context frames keyed inside
//! that runtime instance. All values are NaN-boxed u64.
//!
//! Refcount protocol:
//! - Values stored into a context frame are `inc_ref`'d; values removed are
//!   `dec_ref`'d.
//! - Tokens hold a reference to the old value (if any).

use crate::builtins::numbers::int_bits_from_i64;
use crate::state::runtime_state::{
    runtime_state, ContextVarsState, ContextVarsThreadState, RuntimeState,
};
use crate::{
    dec_ref_bits, inc_ref_bits, obj_from_bits, raise_exception, to_i64, MoltObject, PyToken,
};
use std::thread;

const MISSING: u64 = u64::MAX; // sentinel: no previous value

fn current_thread_state_mut(state: &mut ContextVarsState) -> &mut ContextVarsThreadState {
    state
        .threads
        .entry(thread::current().id())
        .or_insert_with(ContextVarsThreadState::new)
}

pub(crate) fn contextvars_clear_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let old = {
        let mut guard = state.contextvars.lock().unwrap();
        std::mem::replace(&mut *guard, ContextVarsState::new())
    };
    for bits in old.var_defaults.into_values() {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    for thread_state in old.threads.into_values() {
        for frame in thread_state.frames {
            for bits in frame.into_values() {
                dec_ref_bits(_py, bits);
            }
        }
        for (_var_handle, old_bits, used) in thread_state.tokens.into_values() {
            if !used && old_bits != MISSING {
                dec_ref_bits(_py, old_bits);
            }
        }
        for snapshot in thread_state.contexts.into_values() {
            for bits in snapshot.into_values() {
                dec_ref_bits(_py, bits);
            }
        }
    }
}

// ─── Intrinsics ──────────────────────────────────────────────────────────

/// Create a new ContextVar.
/// `name_bits`: NaN-boxed string (ignored for storage, kept for Python repr).
/// `default_bits`: NaN-boxed default value, or None if no default.
/// Returns: NaN-boxed integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_new_var(name_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        // Store the default (inc_ref if not None).
        let default_obj = obj_from_bits(default_bits);
        if !default_obj.is_none() {
            inc_ref_bits(py, default_bits);
        }
        let handle = {
            let mut guard = runtime_state(py).contextvars.lock().unwrap();
            let handle = guard.next_var_handle;
            guard.next_var_handle += 1;
            guard.var_defaults.insert(handle, default_bits);
            handle
        };
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
    crate::with_gil_entry_nopanic!(py, {
        let Some(handle) = to_i64(obj_from_bits(var_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };

        let found = {
            let guard = runtime_state(py).contextvars.lock().unwrap();
            let frame_value = guard
                .threads
                .get(&thread::current().id())
                .and_then(|thread_state| thread_state.frames.last())
                .and_then(|top| top.get(&handle).copied());
            frame_value.or_else(|| {
                guard
                    .var_defaults
                    .get(&handle)
                    .copied()
                    .filter(|bits| !obj_from_bits(*bits).is_none())
            })
        };

        if let Some(bits) = found {
            inc_ref_bits(py, bits);
            return bits;
        }

        raise_exception::<u64>(py, "LookupError", "ContextVar has no value and no default")
    })
}

/// Set the value of a ContextVar in the current context.
/// Returns: NaN-boxed integer token handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_set(var_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(handle) = to_i64(obj_from_bits(var_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };

        let (token_handle, evicted) = {
            let mut guard = runtime_state(py).contextvars.lock().unwrap();
            let token_handle = guard.next_token_handle;
            guard.next_token_handle += 1;
            let thread_state = current_thread_state_mut(&mut guard);
            let top = thread_state
                .frames
                .last_mut()
                .expect("context frame stack empty");
            let token_old = top.get(&handle).copied().unwrap_or(MISSING);
            if token_old != MISSING {
                inc_ref_bits(py, token_old);
            }
            inc_ref_bits(py, value_bits);
            let evicted = top.insert(handle, value_bits);
            thread_state
                .tokens
                .insert(token_handle, (handle, token_old, false));
            (token_handle, evicted)
        };

        // Dec-ref the evicted value (if any).
        if let Some(old) = evicted {
            dec_ref_bits(py, old);
        }

        int_bits_from_i64(py, token_handle)
    })
}

/// Reset a ContextVar to the value it had before the corresponding set().
/// `var_bits`: NaN-boxed integer var handle.
/// `token_bits`: NaN-boxed integer token handle.
/// Returns: None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_reset(var_bits: u64, token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(caller_var) = to_i64(obj_from_bits(var_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };
        let Some(token_handle) = to_i64(obj_from_bits(token_bits)) else {
            return raise_exception::<u64>(py, "TypeError", "ContextVar handle must be an integer");
        };

        let current = {
            let mut guard = runtime_state(py).contextvars.lock().unwrap();
            let Some(thread_state) = guard.threads.get_mut(&thread::current().id()) else {
                return raise_exception::<u64>(py, "ValueError", "Token is invalid");
            };
            let Some((var_handle, old_bits, used)) =
                thread_state.tokens.get(&token_handle).copied()
            else {
                return raise_exception::<u64>(py, "ValueError", "Token is invalid");
            };

            if caller_var != var_handle {
                return raise_exception::<u64>(
                    py,
                    "ValueError",
                    "Token was created by a different ContextVar",
                );
            }

            if used {
                return raise_exception::<u64>(py, "RuntimeError", "Token has already been used");
            }

            let top = thread_state
                .frames
                .last_mut()
                .expect("context frame stack empty");

            // Dec-ref current value.
            let current = top.remove(&var_handle);

            // Restore old value.
            if old_bits != MISSING {
                // The token held a ref; transfer it to the frame.
                top.insert(var_handle, old_bits);
                if let Some(entry) = thread_state.tokens.get_mut(&token_handle) {
                    entry.1 = MISSING;
                }
            }
            if let Some(entry) = thread_state.tokens.get_mut(&token_handle) {
                entry.2 = true;
            }
            current
        };
        if let Some(current) = current {
            dec_ref_bits(py, current);
        }

        MoltObject::none().bits()
    })
}

/// Copy the current context into a new snapshot.
/// Returns: NaN-boxed integer context handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_contextvars_copy_context() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let ctx_handle = {
            let mut guard = runtime_state(py).contextvars.lock().unwrap();
            let ctx_handle = guard.next_context_handle;
            guard.next_context_handle += 1;
            let thread_state = current_thread_state_mut(&mut guard);
            let snapshot = thread_state.frames.last().cloned().unwrap_or_default();
            for &bits in snapshot.values() {
                inc_ref_bits(py, bits);
            }
            thread_state.contexts.insert(ctx_handle, snapshot);
            ctx_handle
        };

        int_bits_from_i64(py, ctx_handle)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exception_pending;

    #[test]
    fn contextvars_state_is_runtime_scoped_and_clearable() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(py, {
            let state = runtime_state(py);
            contextvars_clear_state(py, state);

            let var_bits = molt_contextvars_new_var(
                MoltObject::none().bits(),
                MoltObject::from_int(11).bits(),
            );
            assert_eq!(to_i64(obj_from_bits(var_bits)), Some(1));
            assert_eq!(
                to_i64(obj_from_bits(molt_contextvars_get(var_bits))),
                Some(11)
            );
            assert!(
                state.contextvars.lock().unwrap().threads.is_empty(),
                "default-only reads must not allocate thread context state"
            );

            let token_bits = molt_contextvars_set(var_bits, MoltObject::from_int(22).bits());
            assert_eq!(to_i64(obj_from_bits(token_bits)), Some(1));
            assert_eq!(
                to_i64(obj_from_bits(molt_contextvars_get(var_bits))),
                Some(22)
            );

            let reset_bits = molt_contextvars_reset(var_bits, token_bits);
            assert!(obj_from_bits(reset_bits).is_none());
            assert_eq!(
                to_i64(obj_from_bits(molt_contextvars_get(var_bits))),
                Some(11)
            );

            let ctx_bits = molt_contextvars_copy_context();
            assert_eq!(to_i64(obj_from_bits(ctx_bits)), Some(1));

            {
                let guard = state.contextvars.lock().unwrap();
                assert_eq!(guard.next_var_handle, 2);
                assert_eq!(guard.next_token_handle, 2);
                assert_eq!(guard.next_context_handle, 2);
                assert_eq!(guard.var_defaults.len(), 1);
                let thread_state = guard
                    .threads
                    .get(&std::thread::current().id())
                    .expect("current thread contextvars state should exist");
                assert_eq!(thread_state.contexts.len(), 1);
                let token_handle = to_i64(obj_from_bits(token_bits)).unwrap();
                assert_eq!(
                    thread_state.tokens.get(&token_handle),
                    Some(&(1, MISSING, true))
                );
            }

            contextvars_clear_state(py, state);

            {
                let guard = state.contextvars.lock().unwrap();
                assert_eq!(guard.next_var_handle, 1);
                assert_eq!(guard.next_token_handle, 1);
                assert_eq!(guard.next_context_handle, 1);
                assert!(guard.var_defaults.is_empty());
                assert!(guard.threads.is_empty());
            }
            assert!(!exception_pending(py));
        });
    }
}
