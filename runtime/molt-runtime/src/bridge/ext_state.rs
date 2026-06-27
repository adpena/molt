pub type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
pub type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

pub fn runtime_state_get_or_init(
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, {
        crate::state::runtime_extension_state_get_or_init(
            crate::runtime_state(py),
            key,
            init,
            clear,
            drop,
        )
    })
}

/// # Safety
///
/// Must be called while the target runtime is alive. `bits` must be a cached
/// object handle owned by a runtime extension-state slot.
pub unsafe fn release_runtime_slot_bits(bits: u64) {
    if bits == 0 {
        return;
    }
    crate::with_gil_entry_nopanic!(py, {
        crate::object::release_shutdown_bits(py, bits);
    })
}
