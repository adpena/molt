use std::alloc::{Layout, alloc};
use std::mem::size_of;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BackingCharge {
    alloc_bytes: usize,
    grow_bytes: usize,
}

#[repr(C)]
struct TrackedVecBox<T> {
    charge: BackingCharge,
    vec: Vec<T>,
}

pub(crate) struct TrackedVecOwner<T> {
    ptr: NonNull<TrackedVecBox<T>>,
}

impl<T> Deref for TrackedVecOwner<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        unsafe { &self.ptr.as_ref().vec }
    }
}

impl<T> DerefMut for TrackedVecOwner<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut self.ptr.as_mut().vec }
    }
}

impl<T> Drop for TrackedVecOwner<T> {
    fn drop(&mut self) {
        unsafe {
            let boxed = Box::from_raw(self.ptr.as_ptr());
            let charge = boxed.charge;
            drop(boxed);
            release_grow(charge.grow_bytes);
            release_alloc(charge.alloc_bytes);
        }
    }
}

#[inline]
fn vec_box_bytes<T>() -> usize {
    size_of::<TrackedVecBox<T>>()
}

#[inline]
fn vec_buffer_bytes<T>(capacity: usize) -> Option<usize> {
    capacity.checked_mul(size_of::<T>())
}

#[inline]
fn vec_charge<T>(capacity: usize) -> Option<usize> {
    vec_box_bytes::<T>().checked_add(vec_buffer_bytes::<T>(capacity)?)
}

#[inline]
fn vec_field_offset<T>() -> usize {
    let uninit = std::mem::MaybeUninit::<TrackedVecBox<T>>::uninit();
    let base = uninit.as_ptr();
    unsafe { ptr::addr_of!((*base).vec) as usize - base as usize }
}

#[inline]
unsafe fn owner_ptr_from_vec<T>(ptr: *mut Vec<T>) -> *mut TrackedVecBox<T> {
    unsafe {
        (ptr.cast::<u8>())
            .sub(vec_field_offset::<T>())
            .cast::<TrackedVecBox<T>>()
    }
}

#[inline]
fn charge_alloc(bytes: usize) -> bool {
    if bytes == 0 {
        return true;
    }
    crate::resource::with_tracker(|tracker| tracker.on_allocate(bytes)).is_ok()
}

#[inline]
fn charge_grow(bytes: usize) -> bool {
    if bytes == 0 {
        return true;
    }
    crate::resource::with_tracker(|tracker| tracker.on_grow(bytes)).is_ok()
}

#[inline]
fn release_alloc(bytes: usize) {
    if bytes == 0 {
        return;
    }
    let _ = crate::resource::try_with_tracker(|tracker| tracker.on_free(bytes));
}

#[inline]
fn release_grow(bytes: usize) {
    if bytes == 0 {
        return;
    }
    let _ = crate::resource::try_with_tracker(|tracker| tracker.on_shrink(bytes));
}

/// Allocate an object-owned Vec with resource accounting for both the owning
/// allocation and its backing buffer capacity.
pub(crate) fn tracked_vec_box_with_capacity<T>(capacity: usize) -> Option<*mut Vec<T>> {
    let requested_charge = vec_charge::<T>(capacity)?;
    if !charge_alloc(requested_charge) {
        return None;
    }

    let mut vec = Vec::new();
    if capacity > 0 && vec.try_reserve_exact(capacity).is_err() {
        release_alloc(requested_charge);
        return None;
    }

    let actual_charge = vec_charge::<T>(vec.capacity())?;
    if actual_charge > requested_charge && !charge_grow(actual_charge - requested_charge) {
        drop(vec);
        release_alloc(requested_charge);
        return None;
    }

    let layout = Layout::new::<TrackedVecBox<T>>();
    let raw = unsafe { alloc(layout) as *mut TrackedVecBox<T> };
    if raw.is_null() {
        drop(vec);
        if actual_charge > requested_charge {
            release_grow(actual_charge - requested_charge);
        }
        release_alloc(requested_charge);
        return None;
    }

    unsafe {
        ptr::write(
            raw,
            TrackedVecBox {
                charge: BackingCharge {
                    alloc_bytes: requested_charge,
                    grow_bytes: actual_charge.saturating_sub(requested_charge),
                },
                vec,
            },
        );
        Some(ptr::addr_of_mut!((*raw).vec))
    }
}

pub(crate) fn tracked_vec_box_from_slice<T: Copy>(
    elems: &[T],
    capacity: usize,
) -> Option<*mut Vec<T>> {
    let ptr = tracked_vec_box_with_capacity::<T>(capacity.max(elems.len()))?;
    unsafe {
        (*ptr).extend_from_slice(elems);
    }
    Some(ptr)
}

pub(crate) fn tracked_vec_box_zeroed<T: Copy + Default>(len: usize) -> Option<*mut Vec<T>> {
    let ptr = tracked_vec_box_with_capacity::<T>(len)?;
    unsafe {
        (*ptr).resize(len, T::default());
    }
    Some(ptr)
}

/// Ensure a tracked Vec has capacity for `required_len` elements before a
/// mutating operation grows its length.
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
pub(crate) unsafe fn tracked_vec_reserve_for_len<T>(ptr: *mut Vec<T>, required_len: usize) -> bool {
    if ptr.is_null() {
        return false;
    }
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    let vec = unsafe { &mut *ptr };
    if required_len <= vec.capacity() {
        return true;
    }
    let Some(old_bytes) = vec_buffer_bytes::<T>(vec.capacity()) else {
        return false;
    };
    let Some(target_bytes) = vec_buffer_bytes::<T>(required_len) else {
        return false;
    };
    let Some(delta) = target_bytes.checked_sub(old_bytes) else {
        return false;
    };
    if !charge_grow(delta) {
        return false;
    }
    let additional = required_len.saturating_sub(vec.len());
    if vec.try_reserve_exact(additional).is_err() {
        release_grow(delta);
        return false;
    }
    let Some(actual_bytes) = vec_buffer_bytes::<T>(vec.capacity()) else {
        release_grow(delta);
        return false;
    };
    if actual_bytes > target_bytes {
        let extra = actual_bytes - target_bytes;
        if !charge_grow(extra) {
            release_grow(delta);
            return false;
        }
    } else if actual_bytes < target_bytes {
        release_grow(target_bytes - actual_bytes);
    }
    let actual_delta = actual_bytes.saturating_sub(old_bytes);
    unsafe {
        (*owner).charge.grow_bytes = (*owner).charge.grow_bytes.saturating_add(actual_delta);
    }
    true
}

pub(crate) unsafe fn tracked_vec_reserve_or_raise<T>(
    _py: &crate::PyToken<'_>,
    ptr: *mut Vec<T>,
    required_len: usize,
    message: &str,
) -> bool {
    if unsafe { tracked_vec_reserve_for_len(ptr, required_len) } {
        true
    } else {
        let _ = crate::raise_exception::<u64>(_py, "MemoryError", message);
        false
    }
}

/// Reconstruct the owning tracked Vec allocation from the stored Vec pointer.
///
/// # Safety
/// `ptr` must be a pointer returned by [`tracked_vec_box_with_capacity`]. The
/// returned owner must be dropped exactly once by the caller.
pub(crate) unsafe fn tracked_vec_box_from_raw<T>(ptr: *mut Vec<T>) -> TrackedVecOwner<T> {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    let Some(ptr) = NonNull::new(owner) else {
        std::process::abort();
    };
    TrackedVecOwner { ptr }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{LimitedTracker, ResourceLimits, set_tracker, with_tracker};

    #[test]
    fn tracked_vec_capacity_is_charged_and_released() {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(128),
            ..Default::default()
        })));
        let ptr = tracked_vec_box_with_capacity::<u64>(8).expect("tracked vec");
        let denied = tracked_vec_box_with_capacity::<u64>(64);
        assert!(denied.is_none());
        unsafe {
            drop(tracked_vec_box_from_raw(ptr));
        }
        let ptr = tracked_vec_box_with_capacity::<u64>(8).expect("tracked vec after release");
        unsafe {
            drop(tracked_vec_box_from_raw(ptr));
        }
    }

    #[test]
    fn tracked_vec_growth_uses_grow_not_allocation_count() {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(256),
            max_allocations: Some(1),
            ..Default::default()
        })));
        let ptr = tracked_vec_box_with_capacity::<u64>(1).expect("tracked vec");
        unsafe {
            assert!(tracked_vec_reserve_for_len(ptr, 8));
            (*ptr).push(1);
            drop(tracked_vec_box_from_raw(ptr));
        }
        let result = with_tracker(|tracker| tracker.on_allocate(1));
        assert!(result.is_ok());
    }
}
