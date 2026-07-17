//! FFI bridge for `molt-runtime-asyncio`.

use crate::*;

type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

#[unsafe(no_mangle)]
pub extern "C" fn __molt_asyncio_runtime_state_get_or_init(
    key_ptr: *const u8,
    key_len: usize,
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::runtime_extension_state_get_or_init(
            crate::state::runtime_state::runtime_state(_py),
            key,
            init,
            clear,
            drop,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_asyncio_runtime_state_clear_and_drop(
    key_ptr: *const u8,
    key_len: usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        if crate::state::runtime_state::runtime_extension_state_clear_and_drop_key(
            crate::state::runtime_state::runtime_state(_py),
            key,
        ) {
            1
        } else {
            0
        }
    })
}

pub(crate) fn asyncio_core_clear_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    let _ = _py;
    molt_runtime_asyncio::asyncio_core_clear_state();
}

pub(crate) fn asyncio_queue_clear_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    let _ = _py;
    molt_runtime_asyncio::asyncio_queue_clear_state();
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_asyncio_to_i64(bits: u64, out: *mut i64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_i64(obj) {
        Some(value) => {
            unsafe {
                *out = value;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_asyncio_type_name(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = type_name(_py, obj_from_bits(bits));
        let bytes = name.into_owned().into_bytes().into_boxed_slice();
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering as AtomicOrdering;

    fn ref_count(ptr: *mut u8) -> u32 {
        unsafe {
            (*header_from_obj_ptr(ptr))
                .ref_count
                .load(AtomicOrdering::Relaxed)
        }
    }

    #[test]
    fn asyncio_extension_state_teardown_releases_refs_and_resets_handles() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            crate::state::runtime_extension_states_clear_and_drop(state);

            let future_ptr = alloc_string(_py, b"asyncio-core-clear-owned-ref");
            let future_bits = MoltObject::from_ptr(future_ptr).bits();
            let future_refs_initial = ref_count(future_ptr);
            let future = molt_runtime_asyncio::molt_asyncio_future_new();
            let set_result =
                molt_runtime_asyncio::molt_asyncio_future_set_result_fast(future, future_bits);
            assert_eq!(to_i64(obj_from_bits(set_result)), Some(0));
            assert_eq!(ref_count(future_ptr), future_refs_initial + 1);

            let queue_ptr = alloc_string(_py, b"asyncio-queue-state-item");
            let queue_bits = MoltObject::from_ptr(queue_ptr).bits();
            let queue_refs_initial = ref_count(queue_ptr);
            let queue = molt_runtime_asyncio::molt_asyncio_queue_new(
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
            );
            assert_eq!(to_i64(obj_from_bits(queue)), Some(1));
            assert!(
                obj_from_bits(molt_runtime_asyncio::molt_asyncio_queue_put_nowait(
                    queue, queue_bits,
                ))
                .is_none()
            );
            assert_eq!(ref_count(queue_ptr), queue_refs_initial + 1);

            super::asyncio_core_clear_state(_py);
            super::asyncio_queue_clear_state(_py);
            assert_eq!(ref_count(future_ptr), future_refs_initial);
            assert_eq!(ref_count(queue_ptr), queue_refs_initial);

            let future2 = molt_runtime_asyncio::molt_asyncio_future_new();
            assert_eq!(to_i64(obj_from_bits(future2)), Some(1));
            let queue2 = molt_runtime_asyncio::molt_asyncio_queue_new(
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
            );
            assert_eq!(to_i64(obj_from_bits(queue2)), Some(1));

            crate::state::runtime_extension_states_clear_and_drop(state);
            dec_ref_bits(_py, future_bits);
            dec_ref_bits(_py, queue_bits);
        });
    }
}
