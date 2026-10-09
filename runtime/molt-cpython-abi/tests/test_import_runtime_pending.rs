mod support;

use std::cell::Cell;

use molt_cpython_abi::hooks::{RuntimeHooks, STUB_HOOKS};

// The runtime's raised-exception channel for this test thread. It follows the
// runtime hook contract: a failed import raises into it, clearing empties it,
// and preserved cleanup detaches it around the callback and restores it.
// (A stub that reported "pending" unconditionally could never be drained, so
// every preserved-error cleanup spun forever.)
thread_local! {
    static RUNTIME_PENDING: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "C" fn import_fails(_data: *const u8, _len: usize) -> u64 {
    RUNTIME_PENDING.with(|pending| pending.set(true));
    0
}

unsafe extern "C" fn runtime_exception_pending() -> std::os::raw::c_int {
    RUNTIME_PENDING.with(Cell::get).into()
}

unsafe extern "C" fn runtime_clear_pending_exception() {
    RUNTIME_PENDING.with(|pending| pending.set(false));
}

unsafe extern "C" fn runtime_with_preserved_pending_exception(
    callback: unsafe extern "C" fn(*mut std::ffi::c_void),
    context: *mut std::ffi::c_void,
) {
    let incoming = RUNTIME_PENDING.with(|pending| pending.replace(false));
    unsafe { callback(context) };
    RUNTIME_PENDING.with(|pending| pending.set(incoming));
}

#[test]
fn runtime_import_exception_is_not_masked_by_synthetic_abi_error() {
    let mut hooks: RuntimeHooks = STUB_HOOKS;
    hooks.import_module = import_fails;
    hooks.exception_pending = runtime_exception_pending;
    hooks.clear_pending_exception = runtime_clear_pending_exception;
    hooks.with_preserved_pending_exception = runtime_with_preserved_pending_exception;
    let _abi_test = support::enter_abi_test(hooks);

    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let module =
        unsafe { molt_cpython_abi::api::imports::PyImport_ImportModule(c"numpy.dtypes".as_ptr()) };

    assert!(module.is_null());
    assert!(
        unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "ABI mirror masked the runtime's real pending import exception"
    );
    assert_eq!(
        support::take_current_error_text(),
        None,
        "synthetic ABI message displaced the runtime exception authority"
    );
    assert!(
        RUNTIME_PENDING.with(Cell::get),
        "the runtime's own import exception must stay pending"
    );
    RUNTIME_PENDING.with(|pending| pending.set(false));
}
