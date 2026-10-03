//! C slice construction and observation share the runtime's fixed three slots.
use super::*;

pub(super) unsafe extern "C" fn hook_slice_new(
    start: u64,
    stop: u64,
    step: u64,
) -> OwnedHandleResult {
    with_gil(|py| {
        if crate::exception_pending(&py) {
            return OwnedHandleResult::error();
        }
        let pointer = crate::alloc_slice_obj(&py, start, stop, step);
        if pointer.is_null() {
            if !crate::exception_pending(&py) {
                crate::raise_exception::<()>(&py, "MemoryError", "slice allocation failed");
            }
            return OwnedHandleResult::error();
        }
        OwnedHandleResult::ok(MoltObject::from_ptr(pointer).bits())
    })
}

pub(super) unsafe extern "C" fn hook_slice_item(bits: u64, field: usize) -> BorrowedHandleResult {
    with_gil(|py| unsafe {
        let pointer = MoltObject::from_bits(bits)
            .as_ptr()
            .filter(|pointer| object_type_id(*pointer) == crate::TYPE_ID_SLICE);
        let Some(pointer) = pointer else {
            crate::raise_exception::<()>(&py, "SystemError", "slice field requires an exact slice");
            return BorrowedHandleResult::error();
        };
        let value = match field {
            0 => crate::slice_start_bits(pointer),
            1 => crate::slice_stop_bits(pointer),
            2 => crate::slice_step_bits(pointer),
            _ => {
                crate::raise_exception::<()>(&py, "SystemError", "invalid slice field");
                return BorrowedHandleResult::error();
            }
        };
        BorrowedHandleResult::ok(value)
    })
}
