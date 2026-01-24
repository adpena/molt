//! Handle-resolution entrypoints that bridge NaN-boxed object handles to
//! runtime pointers. This is the ABI surface for resolve paths.

use crate::{
    object::accessors::resolve_obj_ptr, profile_hit_unchecked, HANDLE_RESOLVE_COUNT,
};

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_handle_resolve(bits: u64) -> *mut u8 {
    // GIL-exempt by contract: resolve path must stay read-only and rely on the
    // pointer registry's sharded read locks for safety.
    profile_hit_unchecked(&HANDLE_RESOLVE_COUNT);
    resolve_obj_ptr(bits).unwrap_or(std::ptr::null_mut())
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_handle_resolve(bits: u64) -> u64 {
    // GIL-exempt by contract: resolve path must stay read-only and rely on the
    // pointer registry's sharded read locks for safety.
    profile_hit_unchecked(&HANDLE_RESOLVE_COUNT);
    resolve_obj_ptr(bits).map_or(0, |ptr| ptr as u64)
}

#[cfg(test)]
mod tests {
    use super::molt_handle_resolve;
    use crate::{alloc_object_zeroed_with_pool, object::dec_ref_ptr, MoltHeader, MoltObject, TYPE_ID_OBJECT};

    #[test]
    fn handle_resolve_is_gil_free_for_pointer_bits() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        let total_size = std::mem::size_of::<MoltHeader>() + 8;
        let (ptr, bits) = crate::with_gil_entry!(_py, {
            let ptr = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            (ptr, bits)
        });
        #[cfg(target_arch = "wasm32")]
        {
            let resolved = molt_handle_resolve(bits);
            assert_eq!(resolved, ptr);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let resolved = molt_handle_resolve(bits) as *mut u8;
            assert_eq!(resolved, ptr);
        }
        crate::with_gil_entry!(_py, {
            unsafe { dec_ref_ptr(_py, ptr) };
        });
    }
}
