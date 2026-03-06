pub(crate) fn register_ptr(ptr: *mut u8) -> u64 {
    molt_obj_model::register_ptr(ptr)
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
    use super::{register_ptr, reset_ptr_registry, resolve_ptr};

    #[test]
    fn pointer_registry_resets_entries() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let boxed = Box::new(17_u8);
        let ptr = Box::into_raw(boxed);
        let handle = register_ptr(ptr);
        assert_eq!(resolve_ptr(handle), Some(ptr));
        reset_ptr_registry();
        assert!(resolve_ptr(handle).is_none());
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}
