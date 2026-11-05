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

#[test]
fn public_release_preserves_live_and_dormant_compiled_owners() {
    use crate::c_api::{molt_gil_acquire, molt_gil_release};
    use crate::concurrency::execution::CompiledScopeGuard;
    let _test = crate::test_support::RuntimeTestTransaction::new();
    drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
    assert_eq!(molt_gil_acquire(), 0);
    {
        let execution = RuntimeExecutionGuard::enter();
        let scope = CompiledScopeGuard::enter(execution.token());
        let before = unsafe { hook_foreign_execution_capture() };
        assert_eq!(molt_gil_release(), -1);
        assert_eq!(unsafe { hook_foreign_execution_capture() }, before);
        unsafe {
            let saved = PyEval_SaveThread();
            let tls = crate::state::execution_tls();
            let owner = tls.persistent_execution_owner.get();
            let dependencies = tls.compiled_public_dependencies.get();
            assert_eq!(molt_gil_release(), -1);
            assert_eq!(tls.public_gil_depth.get(), 1);
            assert_eq!(tls.persistent_execution_owner.get(), owner);
            assert_eq!(tls.compiled_public_dependencies.get(), dependencies);
            let callback = PyGILState_Ensure();
            let callback_before = hook_foreign_execution_capture();
            assert_eq!(molt_gil_release(), -1);
            assert_eq!(hook_foreign_execution_capture(), callback_before);
            PyGILState_Release(callback);
            PyEval_RestoreThread(saved);
        }
        scope.validate();
    }
    assert_eq!(molt_gil_release(), 0);
    assert!(!crate::concurrency::execution::has_persistent_runtime_execution());
}

#[test]
fn public_owner_inside_independent_scope_and_native_main_borrow_are_distinct() {
    use crate::c_api::{molt_gil_acquire, molt_gil_release};
    use crate::concurrency::execution::{
        CompiledScopeGuard, ensure_persistent_runtime_execution, has_persistent_runtime_execution,
        release_persistent_runtime_execution,
    };
    let _test = crate::test_support::RuntimeTestTransaction::new();
    drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
    {
        let execution = RuntimeExecutionGuard::enter_with_worker_cleanup();
        let scope = CompiledScopeGuard::enter(execution.token());
        let before = unsafe { hook_foreign_execution_capture() };
        assert_eq!(molt_gil_acquire(), 0);
        assert_eq!(molt_gil_release(), 0);
        assert_eq!(unsafe { hook_foreign_execution_capture() }, before);
        scope.validate();
    }
    ensure_persistent_runtime_execution();
    let owner = crate::state::execution_tls()
        .persistent_execution_owner
        .get();
    assert_ne!(owner, 0);
    assert_eq!(molt_gil_acquire(), 0);
    assert_eq!(molt_gil_release(), 0);
    assert!(has_persistent_runtime_execution());
    assert_eq!(
        crate::state::execution_tls()
            .persistent_execution_owner
            .get(),
        owner
    );
    assert!(release_persistent_runtime_execution());
}

#[test]
fn foreign_return_rejects_missing_restore_before_packed_operand_cleanup() {
    const MODE: &str = "MOLT_FOREIGN_CUSTODY_ADVERSARY";
    const TEST: &str = "cpython_abi_hooks::gil_scope_tests::foreign_return_rejects_missing_restore_before_packed_operand_cleanup";
    if std::env::var_os(MODE).is_some() {
        use molt_cpython_abi::api::cfunction::{
            CFunctionArguments, CFunctionConvention, VectorcallArguments,
        };
        unsafe extern "C" fn invalid_return(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
            let _unrestored = unsafe { PyEval_SaveThread() };
            std::ptr::null_mut()
        }
        let _test = crate::test_support::RuntimeTestTransaction::new();
        let _execution = RuntimeExecutionGuard::enter();
        unsafe {
            CFunctionConvention::VarArgs.invoke(
                invalid_return as *const (),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                CFunctionArguments::Vector(VectorcallArguments {
                    values: &[],
                    positional_count: 0,
                    kwnames: std::ptr::null_mut(),
                }),
                || "foreign custody adversary".into(),
            );
        }
        panic!("invalid foreign return escaped validation");
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(MODE, "missing-restore");
    let output = crate::test_support::captured_runtime_children::capture(
        &mut command,
        "foreign-execution-custody",
        "missing-restore",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "foreign callback unexpectedly returned"
    );
    assert!(
        stderr.contains("foreign callback returned with invalid execution custody"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("invalid foreign return escaped validation"),
        "{stderr}"
    );
}

#[test]
fn foreign_invocation_accepts_balanced_nested_callback_custody() {
    use molt_cpython_abi::api::cfunction::{
        CFunctionArguments, CFunctionConvention, VectorcallArguments,
    };
    unsafe extern "C" fn balanced(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
        unsafe {
            let saved = PyEval_SaveThread();
            let callback = PyGILState_Ensure();
            PyGILState_Release(callback);
            PyEval_RestoreThread(saved);
        }
        std::ptr::null_mut()
    }
    let _test = crate::test_support::RuntimeTestTransaction::new();
    let _execution = RuntimeExecutionGuard::enter();
    let before = unsafe { hook_foreign_execution_capture() };
    let result = unsafe {
        CFunctionConvention::VarArgs.invoke(
            balanced as *const (),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CFunctionArguments::Vector(VectorcallArguments {
                values: &[],
                positional_count: 0,
                kwnames: std::ptr::null_mut(),
            }),
            || "balanced foreign callback".into(),
        )
    };
    assert!(result.is_null());
    assert_eq!(unsafe { hook_foreign_execution_capture() }, before);
}

#[test]
fn root_foreign_callouts_reject_missing_restore_before_return_cleanup() {
    const MODE: &str = "MOLT_ROOT_FOREIGN_CALLOUT_ADVERSARY";
    const TEST: &str = "cpython_abi_hooks::gil_scope_tests::root_foreign_callouts_reject_missing_restore_before_return_cleanup";
    const MODES: &[&str] = &[
        "pyinit",
        "module-traverse",
        "module-clear",
        "module-free",
        "heap-export",
        "heap-release",
        "heap-invalid-view-release",
    ];
    if let Ok(mode) = std::env::var(MODE) {
        use molt_cpython_abi::abi_types::PyModuleDef;
        use std::ffi::c_void;
        unsafe fn lose_custody() {
            eprintln!("root foreign callout executed");
            let _saved = unsafe { PyEval_SaveThread() };
        }
        unsafe extern "C" fn init() -> *mut PyObject {
            unsafe { lose_custody() };
            std::ptr::null_mut()
        }
        unsafe extern "C" fn traverse(_: *mut PyObject, _: *mut c_void, _: *mut c_void) -> i32 {
            unsafe { lose_custody() };
            0
        }
        unsafe extern "C" fn clear(_: *mut PyObject) -> i32 {
            unsafe { lose_custody() };
            0
        }
        unsafe extern "C" fn free(_: *mut c_void) -> i32 {
            unsafe { lose_custody() };
            0
        }
        unsafe extern "C" fn export(_: usize, _: *mut crate::MoltBufferView) -> i32 {
            unsafe { lose_custody() };
            -1
        }
        unsafe extern "C" fn release(_: usize, _: *mut crate::MoltBufferView) -> i32 {
            unsafe { lose_custody() };
            0
        }
        static DATA: [u8; 1] = [42];
        unsafe extern "C" fn valid_export(_: usize, view: *mut crate::MoltBufferView) -> i32 {
            unsafe {
                *view = crate::MoltBufferView::default();
                (*view).data = DATA.as_ptr().cast_mut();
                (*view).len = 1;
                (*view).backing_capacity = 1;
                (*view).readonly = 1;
                (*view).itemsize = 1;
                (*view).ndim = 1;
                (*view).shape[0] = 1;
                (*view).strides[0] = 1;
                (*view).format[0] = b'B';
            }
            0
        }
        unsafe extern "C" fn invalid_view(_: usize, view: *mut crate::MoltBufferView) -> i32 {
            unsafe { *view = crate::MoltBufferView::default() };
            0
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = RuntimeExecutionGuard::enter();
        let py = execution.token();
        unsafe {
            if mode == "pyinit" {
                let name =
                    MoltObject::from_ptr(crate::alloc_string(py, b"custody_boundary")).bits();
                extension_init::hook_initialize_extension(
                    init,
                    name,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    false,
                );
            } else if mode.starts_with("module-") {
                let mut definition: PyModuleDef = std::mem::zeroed();
                definition.m_base.ob_base.ob_refcnt = 1;
                definition.m_name = c"custody_module".as_ptr();
                definition.m_size = 0;
                match mode.as_str() {
                    "module-traverse" => {
                        definition.m_traverse = traverse as *const () as *mut c_void
                    }
                    "module-clear" => definition.m_clear = clear as *const () as *mut c_void,
                    "module-free" => definition.m_free = free as *const () as *mut c_void,
                    _ => unreachable!(),
                }
                let module =
                    molt_cpython_abi::api::modules::PyModule_Create2(&raw mut definition, 1013);
                assert!(!module.is_null());
                let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .managed_handle_for_pyobj(module)
                    .unwrap();
                let pointer = crate::obj_from_bits(bits).as_ptr().unwrap();
                match mode.as_str() {
                    "module-traverse" => {
                        crate::c_api::c_api_module_visit_native_edges(py, pointer, &mut |_| {});
                    }
                    "module-clear" => {
                        crate::c_api::c_api_module_clear_native_edges(py, pointer);
                    }
                    "module-free" => {
                        molt_cpython_abi::api::refcount::Py_DECREF(module);
                    }
                    _ => unreachable!(),
                }
            } else {
                // This is the public C-heap header contract, populated and
                // admitted by the production canonicalization/registration APIs.
                #[repr(C)]
                struct Header {
                    magic: u64,
                    refcnt: u32,
                    kind: u32,
                    type_ptr: usize,
                    dealloc: usize,
                }
                const TYPE: u32 = 0x6454_5401;
                const OBJECT: u32 = 0x6454_4101;
                let mut ty = Header {
                    magic: 0x4d4f4c54434f424a,
                    refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
                    kind: TYPE,
                    type_ptr: 0,
                    dealloc: 0,
                };
                let type_ptr = (&raw mut ty) as usize;
                ty.type_ptr = type_ptr;
                assert_eq!(
                    crate::c_api::molt_c_heap_type_canonicalize(TYPE, type_ptr),
                    type_ptr
                );
                let mut object = Header {
                    magic: ty.magic,
                    refcnt: 1,
                    kind: OBJECT,
                    type_ptr,
                    dealloc: 0,
                };
                let object_ptr = (&raw mut object) as usize;
                assert_eq!(crate::c_api::molt_c_heap_register(object_ptr), 0);
                let exporter = match mode.as_str() {
                    "heap-export" => export,
                    "heap-release" => valid_export,
                    "heap-invalid-view-release" => invalid_view,
                    _ => unreachable!(),
                };
                assert_eq!(
                    crate::c_api::molt_c_heap_register_buffer_exporter(
                        OBJECT,
                        type_ptr,
                        Some(exporter)
                    ),
                    0
                );
                assert_eq!(
                    crate::c_api::molt_c_heap_register_buffer_releaser(
                        OBJECT,
                        type_ptr,
                        Some(release)
                    ),
                    0
                );
                let mut view = crate::MoltBufferView::default();
                let status = crate::c_api::molt_c_heap_export_buffer_attached_body(
                    py,
                    object_ptr,
                    &raw mut view,
                );
                if mode == "heap-release" {
                    assert_eq!(status, 0);
                    crate::c_api::molt_c_heap_release_buffer_attached_body(
                        py,
                        object_ptr,
                        &raw mut view,
                    );
                }
            }
        }
        panic!("root foreign callout escaped immediate custody validation");
    }
    for mode in MODES {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(MODE, mode);
        let output = crate::test_support::captured_runtime_children::capture(
            &mut command,
            "root-foreign-callout-custody",
            mode,
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{mode}: {stderr}");
        assert!(
            stderr.contains("root foreign callout executed"),
            "{mode}: {stderr}"
        );
        assert!(
            stderr.contains("foreign callback returned with invalid execution custody"),
            "{mode}: {stderr}"
        );
        assert!(
            !stderr.contains("escaped immediate custody validation"),
            "{mode}: {stderr}"
        );
    }
}
