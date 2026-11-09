use super::*;
use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

struct AllocationDenial;

impl AllocationDenial {
    fn enter() -> Self {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(0),
            ..Default::default()
        })));
        Self
    }
}

impl Drop for AllocationDenial {
    fn drop(&mut self) {
        set_tracker(Box::new(UnlimitedTracker));
    }
}

#[test]
fn public_gil_ensure_exposes_types_before_foreign_control_returns() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| unsafe {
        let kind = &raw mut molt_cpython_abi::abi_types::PyExc_ZeroDivisionError;
        assert!((*kind).tp_dict.is_null());
        let state = molt_cpython_abi::api::object::PyGILState_Ensure();
        assert!(!(*kind).tp_dict.is_null());
        assert!(!(*kind).tp_bases.is_null());
        assert!(!(*kind).tp_mro.is_null());
        molt_cpython_abi::api::object::PyGILState_Release(state);
    });
}

#[test]
fn c_type_exposure_failure_keeps_hooks_live_and_can_retry() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        with_gil(|py| unsafe {
            let runtime = crate::runtime_state(py);
            assert!(
                molt_cpython_abi::abi_types::PyExc_ZeroDivisionError
                    .tp_dict
                    .is_null()
            );
            let hooks = molt_cpython_abi::hooks::hooks().expect("bootstrap published hooks");
            assert_eq!((hooks.runtime_is_initialized)(), 1);
            {
                let _denial = AllocationDenial::enter();
                assert!(!expose_cpython_types());
            }
            assert_eq!((hooks.runtime_is_initialized)(), 1);
            assert!(crate::exception_pending(py));
            assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            assert_eq!(
                *runtime.cpython.registration.lock().unwrap(),
                HookRegistrationState::Prepared
            );
            molt_cpython_abi::api::errors::PyErr_Clear();
            crate::clear_exception(py);
            assert!(expose_cpython_types());
            for pointer in molt_cpython_abi::abi_types::type_static_ptrs()
                .chain(molt_cpython_abi::abi_types::exc_singleton_ptrs())
            {
                let ty = pointer.cast::<PyTypeObject>();
                assert!(!(*ty).tp_dict.is_null());
                assert!(!(*ty).tp_mro.is_null());
                assert!(!(*ty).tp_bases.is_null());
            }
            assert!(!crate::exception_pending(py));
        });
    });
}
