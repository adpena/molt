//! Canonical owned transfer of the current raised exception.

use super::*;

#[derive(Copy, Clone)]
pub(crate) enum RaisedSnapshot {
    None,
    Thread(u64),
    Task(PtrSlot, u64),
    Emergency(PtrSlot),
}

pub(crate) fn take_raised(_py: &PyToken<'_>) -> RaisedSnapshot {
    if emergency_memory_error_pending_for_current() {
        let owner = PtrSlot(current_task_ptr());
        clear_emergency_memory_error_for_current();
        CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(false));
        return RaisedSnapshot::Emergency(owner);
    }
    let raised = if let Some(key) = current_task_key() {
        let mut guard = task_last_exceptions(_py)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(&key).copied() {
            None => RaisedSnapshot::None,
            Some(slot) => {
                assert!(
                    exception_slot_is_valid(slot),
                    "owned task exception slot must reference a live exception"
                );
                let removed = guard
                    .remove(&key)
                    .expect("validated task exception slot must remain present");
                RaisedSnapshot::Task(key, MoltObject::from_ptr(removed.0).bits())
            }
        }
    } else {
        match thread_last_exception_raw_slot() {
            None => RaisedSnapshot::None,
            Some(slot) => {
                assert!(
                    exception_slot_is_valid(slot),
                    "owned thread exception slot must reference a live exception"
                );
                let removed = thread_last_exception_take()
                    .expect("validated thread exception slot must remain present");
                RaisedSnapshot::Thread(MoltObject::from_ptr(removed.0).bits())
            }
        }
    };
    CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(false));
    raised
}

pub(super) fn release_raised(_py: &PyToken<'_>, saved: &mut Option<RaisedSnapshot>) {
    let Some(saved) = saved.take() else {
        return;
    };
    match saved {
        RaisedSnapshot::Thread(bits) | RaisedSnapshot::Task(_, bits)
            if !obj_from_bits(bits).is_none() =>
        {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, bits));
        }
        RaisedSnapshot::None
        | RaisedSnapshot::Emergency(_)
        | RaisedSnapshot::Thread(_)
        | RaisedSnapshot::Task(_, _) => {}
    }
}

pub(crate) fn resolve_raised(_py: &PyToken<'_>, saved: &mut Option<RaisedSnapshot>) {
    let Some(saved_snapshot) = *saved else {
        return;
    };
    match saved_snapshot {
        RaisedSnapshot::None => {}
        RaisedSnapshot::Emergency(owner) => {
            THREAD_LAST_EXCEPTION.with(|state| state.set_emergency_memory_error(owner.0));
            *saved = None;
            if current_task_ptr() == owner.0 {
                CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
            }
        }
        RaisedSnapshot::Thread(bits) => {
            if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                let old = THREAD_LAST_EXCEPTION.with(|slot| slot.replace(ptr));
                // Publication transfers the saved strong edge to TLS.
                *saved = None;
                if current_task_key().is_none() {
                    CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
                }
                if !old.is_null() {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, MoltObject::from_ptr(old).bits());
                    });
                }
            }
        }
        RaisedSnapshot::Task(key, bits) => {
            if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                let mut guard = task_last_exceptions(_py)
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let old = guard.insert(key, PtrSlot(ptr));
                drop(guard);
                // Map publication transfers the saved strong edge.
                *saved = None;
                if current_task_key() == Some(key) {
                    CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(true));
                }
                if let Some(old) = old {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, MoltObject::from_ptr(old.0).bits());
                    });
                }
            }
        }
    }
    // None or an invalid non-object carries no strong edge.
    if saved.is_some() {
        *saved = None;
    }
}

pub(super) fn discard_current_raised(_py: &PyToken<'_>) {
    // Use the transaction's poison-tolerant detach path rather than the public
    // clear helper: unraisable cleanup must remain available after a task-map
    // panic poisoned its mutex.
    let mut raised = Some(take_raised(_py));
    release_raised(_py, &mut raised);
}

/// Run a fallible callback boundary with no incoming raised exception visible.
/// Success restores the exact incoming owner; failure keeps the callback's
/// exception and releases the incoming owner. Handled exception state stays live.
pub(crate) fn with_saved_raised_exception(py: &PyToken<'_>, run: impl FnOnce() -> bool) -> bool {
    let mut saved = Some(take_raised(py));
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(true) => {
            resolve_raised(py, &mut saved);
            true
        }
        Ok(false) => {
            release_raised(py, &mut saved);
            false
        }
        Err(payload) => {
            discard_current_raised(py);
            resolve_raised(py, &mut saved);
            std::panic::resume_unwind(payload)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoring_same_exception_releases_the_replaced_owner() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            let ptr = alloc_exception(py, "ValueError", "same owner");
            assert!(!ptr.is_null());
            let refs = || unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() };
            let baseline = refs();
            record_exception(py, ptr);
            let mut saved = Some(take_raised(py));
            record_exception(py, ptr);
            assert_eq!(refs(), baseline + 2);
            resolve_raised(py, &mut saved);
            assert!(saved.is_none());
            assert_eq!(refs(), baseline + 1);
            clear_exception(py);
            assert_eq!(refs(), baseline);
            dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn callback_boundary_restores_or_replaces_emergency_raised_state() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            record_memory_error_without_allocation(py);
            assert!(with_saved_raised_exception(py, || {
                assert!(!exception_pending(py));
                assert!(!emergency_memory_error_pending_for_current());
                true
            }));
            assert!(emergency_memory_error_pending_for_current());
            assert!(exception_pending(py));
            assert!(!with_saved_raised_exception(py, || {
                raise_exception::<u64>(py, "ValueError", "replacement");
                false
            }));
            assert!(!emergency_memory_error_pending_for_current());
            let raised = molt_exception_last_pending();
            assert!(exception_matches_builtin_name(py, raised, "ValueError"));
            clear_exception(py);
            dec_ref_bits(py, raised);
            assert!(!exception_pending(py));
        });
    }
}
