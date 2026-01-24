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
