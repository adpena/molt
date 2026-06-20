use std::mem::size_of;

fn bridge_buffer_bytes<T>(len: usize) -> Option<usize> {
    len.checked_mul(size_of::<T>())
}

fn charge_bridge_buffer<T>(len: usize) -> bool {
    let Some(bytes) = bridge_buffer_bytes::<T>(len) else {
        return false;
    };
    if bytes == 0 {
        return true;
    }
    crate::resource::with_tracker(|tracker| tracker.on_allocate(bytes)).is_ok()
}

fn release_bridge_buffer<T>(len: usize) {
    let Some(bytes) = bridge_buffer_bytes::<T>(len) else {
        return;
    };
    if bytes == 0 {
        return;
    }
    let _ = crate::resource::try_with_tracker(|tracker| tracker.on_free(bytes));
}

pub(crate) fn export_u8_box(bytes: Box<[u8]>, out_ptr: *mut *const u8, out_len: *mut usize) -> i32 {
    export_box(bytes, out_ptr, out_len)
}

pub(crate) fn export_u64_box(
    values: Box<[u64]>,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    export_box(values, out_ptr, out_len)
}

#[cfg(feature = "stdlib_compression")]
pub(crate) fn export_u8_box_ptr(bytes: Box<[u8]>, out_len: *mut usize) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    let len = bytes.len();
    if !charge_bridge_buffer::<u8>(len) {
        unsafe {
            *out_len = 0;
        }
        return std::ptr::null_mut();
    }
    let ptr = Box::into_raw(bytes) as *mut u8;
    unsafe {
        *out_len = len;
    }
    ptr
}

fn export_box<T>(boxed: Box<[T]>, out_ptr: *mut *const T, out_len: *mut usize) -> i32 {
    if out_ptr.is_null() || out_len.is_null() {
        return 0;
    }
    let len = boxed.len();
    if !charge_bridge_buffer::<T>(len) {
        return 0;
    }
    let ptr = Box::into_raw(boxed) as *const T;
    unsafe {
        *out_ptr = ptr;
        *out_len = len;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_bridge_free_u8(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
    release_bridge_buffer::<u8>(len);
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_bridge_free_u64(ptr: *mut u64, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
    release_bridge_buffer::<u64>(len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{
        LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker, with_tracker,
    };

    struct TrackerReset;

    impl Drop for TrackerReset {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    #[test]
    fn bridge_buffer_export_charges_and_free_releases() {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(8),
            ..Default::default()
        })));
        let _reset = TrackerReset;
        let mut out_ptr: *const u8 = std::ptr::null();
        let mut out_len = 0usize;
        assert_eq!(
            export_u8_box(Box::from([1u8, 2, 3, 4]), &mut out_ptr, &mut out_len),
            1
        );
        assert_eq!(out_len, 4);
        assert!(!out_ptr.is_null());
        assert!(with_tracker(|tracker| tracker.on_allocate(5)).is_err());
        __molt_bridge_free_u8(out_ptr as *mut u8, out_len);
        assert!(with_tracker(|tracker| tracker.on_allocate(8)).is_ok());
    }

    #[test]
    fn bridge_buffer_export_denies_without_leaking_charge() {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(4),
            ..Default::default()
        })));
        let _reset = TrackerReset;
        let mut out_ptr: *const u64 = std::ptr::null();
        let mut out_len = 0usize;
        assert_eq!(
            export_u64_box(Box::from([1u64]), &mut out_ptr, &mut out_len),
            0
        );
        assert!(out_ptr.is_null());
        assert_eq!(out_len, 0);
        assert!(with_tracker(|tracker| tracker.on_allocate(4)).is_ok());
    }
}
