#[cfg(target_os = "macos")]
pub(crate) fn release_native_backend_batch_memory_to_os() {
    unsafe extern "C" {
        fn malloc_default_zone() -> *mut libc::c_void;
        fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: usize) -> usize;
    }

    unsafe {
        let zone = malloc_default_zone();
        if !zone.is_null() {
            let _ = malloc_zone_pressure_relief(zone, usize::MAX);
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn release_native_backend_batch_memory_to_os() {
    unsafe {
        let _ = libc::malloc_trim(0);
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
pub(crate) fn release_native_backend_batch_memory_to_os() {}
