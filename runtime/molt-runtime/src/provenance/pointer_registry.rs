use crate::MoltObject;

pub(crate) fn opaque_handle_bits(ptr: *mut u8) -> u64 {
    let addr = molt_obj_model::register_ptr(ptr);
    debug_assert!(
        addr <= ((1_u64 << 46) - 1),
        "opaque runtime handle address exceeds Molt immediate int range"
    );
    MoltObject::from_int(addr as i64).bits()
}

#[cfg(not(feature = "stdlib_ipaddress"))]
pub(crate) fn opaque_handle_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let addr = MoltObject::from_bits(bits).as_int()?;
    if addr < 0 {
        return None;
    }
    molt_obj_model::resolve_ptr(addr as u64)
}

pub(crate) fn resolve_ptr(addr: u64) -> Option<*mut u8> {
    molt_obj_model::resolve_ptr(addr)
}

pub(crate) fn release_ptr(ptr: *mut u8) -> Option<u64> {
    molt_obj_model::release_ptr(ptr)
}

pub(crate) fn reset_ptr_registry() {
    molt_obj_model::reset_ptr_registry();
}

#[cfg(test)]
mod tests {
    use super::{reset_ptr_registry, resolve_ptr};

    #[test]
    fn pointer_registry_resets_entries() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let boxed = Box::new(17_u8);
        let ptr = Box::into_raw(boxed);
        let handle = molt_obj_model::register_ptr(ptr);
        assert_eq!(resolve_ptr(handle), Some(ptr));
        reset_ptr_registry();
        assert!(resolve_ptr(handle).is_none());
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}
