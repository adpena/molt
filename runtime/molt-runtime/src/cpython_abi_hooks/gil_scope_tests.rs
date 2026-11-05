use super::*;
use molt_cpython_abi::api::object::{
    _PyThreadState_UncheckedGet, PyEval_RestoreThread, PyEval_SaveThread, PyGILState_Ensure,
    PyGILState_Release, PyThreadState_Get, PyThreadState_GetID,
};

#[test]
fn saved_thread_callbacks_own_independent_nested_gil_scopes() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    // The test transaction may retain an initialization record. End that real
    // boundary first so the next WorkerEnd owns exactly the record it creates.
    drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
    let retained_before = molt_cpython_abi::api::object::runtime_retained_thread_state_count();
    let attached_before = molt_cpython_abi::api::object::runtime_execution_attachment_count();
    {
        let _execution = RuntimeExecutionGuard::enter_with_worker_cleanup();
        unsafe {
            let original = PyThreadState_Get();
            let identity = PyThreadState_GetID(original);
            let outer_saved = PyEval_SaveThread();
            assert!(_PyThreadState_UncheckedGet().is_null());
            let callback = PyGILState_Ensure();
            assert_eq!(PyThreadState_GetID(PyThreadState_Get()), identity);
            let inner_saved = PyEval_SaveThread();
            assert!(_PyThreadState_UncheckedGet().is_null());
            let nested = PyGILState_Ensure();
            assert_eq!(PyThreadState_GetID(PyThreadState_Get()), identity);
            PyGILState_Release(nested);
            assert!(_PyThreadState_UncheckedGet().is_null());
            PyEval_RestoreThread(inner_saved);
            assert_eq!(PyThreadState_Get(), original);
            PyGILState_Release(callback);
            assert!(_PyThreadState_UncheckedGet().is_null());
            PyEval_RestoreThread(outer_saved);
            assert_eq!(PyThreadState_Get(), original);
        }
    }
    assert!(!gil_owned_by_current_thread());
    assert_eq!(
        molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
        retained_before
    );
    assert_eq!(
        molt_cpython_abi::api::object::runtime_execution_attachment_count(),
        attached_before
    );
    ABI_GIL_ENSURE_STATE.with(|slot| assert!(slot.borrow().is_empty()));
    ABI_GIL_RELEASE_STATE.with(|slot| assert!(slot.borrow().is_empty()));
}
