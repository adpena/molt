//! Canonical owned transfer of the current raised exception.

use super::*;

#[derive(Copy, Clone)]
pub(crate) enum RaisedSnapshot {
    None,
    Thread(u64),
    Task(PtrSlot, u64),
}

pub(crate) fn take_raised(_py: &PyToken<'_>) -> RaisedSnapshot {
    let raised = if let Some(key) = current_task_key() {
        match replace_task_raised_exception(key, None) {
            None => RaisedSnapshot::None,
            Some(slot) => {
                assert!(
                    exception_slot_is_valid(slot),
                    "owned task exception slot must reference a live exception"
                );
                RaisedSnapshot::Task(key, MoltObject::from_ptr(slot.0).bits())
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
    exception_pending_flag().set(false);
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
            dec_ref_bits(_py, bits);
        }
        RaisedSnapshot::None | RaisedSnapshot::Thread(_) | RaisedSnapshot::Task(_, _) => {}
    }
}

pub(crate) fn resolve_raised(_py: &PyToken<'_>, saved: &mut Option<RaisedSnapshot>) {
    let Some(saved_snapshot) = *saved else {
        return;
    };
    match saved_snapshot {
        RaisedSnapshot::None => {}
        RaisedSnapshot::Thread(bits) => {
            if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                let old = THREAD_LAST_EXCEPTION.with(|slot| slot.replace(ptr));
                // Publication transfers the saved strong edge to TLS.
                *saved = None;
                if current_task_key().is_none() {
                    exception_pending_flag().set(true);
                }
                if !old.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(old).bits());
                }
            }
        }
        RaisedSnapshot::Task(key, bits) => {
            if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                let old = replace_task_raised_exception(key, Some(PtrSlot(ptr)));
                // Publication transfers the saved strong edge to the task.
                *saved = None;
                if current_task_key() == Some(key) {
                    exception_pending_flag().set(true);
                }
                if let Some(old) = old {
                    dec_ref_bits(_py, MoltObject::from_ptr(old.0).bits());
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
    let mut raised = Some(take_raised(_py));
    molt_cpython_abi::api::errors::with_preserved_error(|| release_raised(_py, &mut raised));
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
            let ptr = alloc_exception(py, crate::BuiltinExc::ValueError, "same owner");
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

    /// A thread that exits with a pending exception releases it from a
    /// thread-local destructor. The terminal DECREF probes finalizers through
    /// the preserved-error boundary, which re-enters the pending-exception
    /// slot; that must not abort the process.
    #[test]
    fn thread_exit_releases_pending_exception_through_finalizer_probe() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        std::thread::spawn(|| {
            crate::with_gil_entry_nopanic!(py, {
                let ptr =
                    alloc_exception(py, crate::BuiltinExc::ValueError, "pending at thread exit");
                assert!(!ptr.is_null());
                record_exception_owned(py, ptr);
                assert!(exception_pending(py));
            });
        })
        .join()
        .expect("thread exit releases its pending exception");
    }

    #[test]
    fn callback_boundary_restores_or_replaces_emergency_raised_state() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            record_memory_error_without_allocation(py);
            let original = exception_last_bits_noinc(py).expect("reserved MemoryError");
            assert!(with_saved_raised_exception(py, || {
                assert!(!exception_pending(py));
                true
            }));
            assert_eq!(exception_last_bits_noinc(py), Some(original));
            assert!(exception_pending(py));
            assert!(!with_saved_raised_exception(py, || {
                raise_exception::<u64>(py, crate::BuiltinExc::ValueError, "replacement");
                false
            }));
            assert_ne!(exception_last_bits_noinc(py), Some(original));
            let raised = molt_exception_last_pending_attached_body(py);
            assert!(exception_matches_kind(
                py,
                raised,
                crate::BuiltinExc::ValueError
            ));
            clear_exception(py);
            dec_ref_bits(py, raised);
            assert!(!exception_pending(py));
        });
    }
}
