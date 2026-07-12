use molt_cpython_abi::hooks::{RuntimeHooks, STUB_HOOKS};

unsafe extern "C" fn import_fails(_data: *const u8, _len: usize) -> u64 {
    0
}

unsafe extern "C" fn runtime_exception_pending() -> std::os::raw::c_int {
    1
}

#[test]
fn runtime_import_exception_is_not_masked_by_synthetic_abi_error() {
    let mut hooks: RuntimeHooks = STUB_HOOKS;
    hooks.import_module = import_fails;
    hooks.exception_pending = runtime_exception_pending;
    assert!(
        unsafe { molt_cpython_abi::try_set_runtime_hooks(hooks) },
        "install runtime hooks"
    );

    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let module = unsafe {
        molt_cpython_abi::api::imports::PyImport_ImportModule(c"numpy.dtypes".as_ptr())
    };

    assert!(module.is_null());
    assert!(
        unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "ABI mirror masked the runtime's real pending import exception"
    );
    assert_eq!(
        molt_cpython_abi::api::errors::take_current_error_message(),
        None,
        "synthetic ABI message displaced the runtime exception authority"
    );
}
