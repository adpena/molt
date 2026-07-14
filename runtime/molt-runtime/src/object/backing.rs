use std::alloc::{Layout, alloc};
use std::mem::size_of;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BackingCharge {
    owner_bytes: usize,
    buffer_bytes: usize,
}

#[repr(C)]
struct TrackedVecBox<T> {
    charge: BackingCharge,
    /// Monotonic publication generation for object-owned mutable sequence
    /// storage. Ordinary runtime execution serializes mutation with the GIL;
    /// the atomic generation is the lock-free observation seam for callback
    /// reentrancy and the future free-threaded object-lock path.
    mutation_epoch: AtomicU64,
    mutation_lock: AtomicBool,
    /// Exact heap-reference edge count for generic list/tuple storage. Other
    /// tracked Vec owners leave this at zero. Keeping it beside the stable
    /// mutation generation removes full-sequence rescans from scalar mutation
    /// hot paths.
    heap_edge_count: AtomicUsize,
    vec: Vec<T>,
}

pub(crate) struct TrackedVecMutationGuard<T> {
    owner: NonNull<TrackedVecBox<T>>,
}

impl<T> Drop for TrackedVecMutationGuard<T> {
    fn drop(&mut self) {
        unsafe {
            self.owner
                .as_ref()
                .mutation_lock
                .store(false, Ordering::Release);
        }
    }
}

pub(crate) struct TrackedVecOwner<T> {
    ptr: NonNull<TrackedVecBox<T>>,
}

/// Detached backing-buffer custody from a still-live tracked Vec owner. The
/// stable owner retains its lock/epoch identity with an empty zero-charge Vec;
/// dropping this value releases the displaced buffer's resource charge.
pub(crate) struct TrackedVecContents<T> {
    vec: Vec<T>,
    buffer_bytes: usize,
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
            release_alloc(charge.owner_bytes.saturating_add(charge.buffer_bytes));
        }
    }
}

impl<T> Deref for TrackedVecContents<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.vec
    }
}

impl<T> DerefMut for TrackedVecContents<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.vec
    }
}

impl<T> Drop for TrackedVecContents<T> {
    fn drop(&mut self) {
        release_alloc(self.buffer_bytes);
    }
}

#[inline]
fn vec_owner_bytes<T>() -> usize {
    size_of::<TrackedVecBox<T>>()
}

#[inline]
fn vec_buffer_bytes<T>(capacity: usize) -> Option<usize> {
    capacity.checked_mul(size_of::<T>())
}

#[inline]
fn vec_charge<T>(capacity: usize) -> Option<usize> {
    vec_owner_bytes::<T>().checked_add(vec_buffer_bytes::<T>(capacity)?)
}

#[inline]
fn amortized_target_capacity(current_capacity: usize, required_len: usize) -> usize {
    required_len.max(current_capacity.saturating_mul(2)).max(4)
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
pub(crate) fn charge_alloc(bytes: usize) -> bool {
    if bytes == 0 {
        return true;
    }
    crate::resource::with_tracker(|tracker| tracker.on_allocate(bytes)).is_ok()
}

#[inline]
pub(crate) fn charge_grow(bytes: usize) -> bool {
    if bytes == 0 {
        return true;
    }
    crate::resource::with_tracker(|tracker| tracker.on_grow(bytes)).is_ok()
}

#[inline]
pub(crate) fn release_alloc(bytes: usize) {
    if bytes == 0 {
        return;
    }
    let _ = crate::resource::try_with_tracker(|tracker| tracker.on_free(bytes));
}

#[inline]
pub(crate) fn release_grow(bytes: usize) {
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

    let Some(actual_charge) = vec_charge::<T>(vec.capacity()) else {
        drop(vec);
        release_alloc(requested_charge);
        return None;
    };
    let Some(actual_buffer_bytes) = vec_buffer_bytes::<T>(vec.capacity()) else {
        drop(vec);
        if actual_charge > requested_charge {
            release_grow(actual_charge - requested_charge);
        }
        release_alloc(requested_charge);
        return None;
    };
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
                    owner_bytes: vec_owner_bytes::<T>(),
                    buffer_bytes: actual_buffer_bytes,
                },
                mutation_epoch: AtomicU64::new(0),
                mutation_lock: AtomicBool::new(false),
                heap_edge_count: AtomicUsize::new(0),
                vec,
            },
        );
        Some(ptr::addr_of_mut!((*raw).vec))
    }
}

/// Acquire the stable mutation lock associated with an object-owned vector.
/// The ordinary deterministic runtime path is serialized by the GIL, so this
/// is uncontended today; the same lock becomes the per-object publication
/// boundary for free-threaded execution.
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
pub(crate) unsafe fn tracked_vec_mutation_lock<T>(ptr: *mut Vec<T>) -> TrackedVecMutationGuard<T> {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    loop {
        let acquired = unsafe {
            (*owner)
                .mutation_lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        };
        if acquired {
            return TrackedVecMutationGuard {
                owner: unsafe { NonNull::new_unchecked(owner) },
            };
        }
        std::hint::spin_loop();
    }
}

/// Return the current publication generation for tracked sequence storage.
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
#[inline]
pub(crate) unsafe fn tracked_vec_mutation_epoch<T>(ptr: *mut Vec<T>) -> u64 {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).mutation_epoch.load(Ordering::Acquire) }
}

/// Publish one in-place mutation and return the new generation.
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
#[inline]
pub(crate) unsafe fn tracked_vec_bump_mutation_epoch<T>(ptr: *mut Vec<T>) -> u64 {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe {
        (*owner)
            .mutation_epoch
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }
}

#[inline]
pub(crate) unsafe fn tracked_vec_heap_edge_count<T>(ptr: *mut Vec<T>) -> usize {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).heap_edge_count.load(Ordering::Acquire) }
}

#[inline]
pub(crate) unsafe fn tracked_vec_set_heap_edge_count<T>(ptr: *mut Vec<T>, count: usize) {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).heap_edge_count.store(count, Ordering::Release) };
}

#[inline]
pub(crate) unsafe fn tracked_vec_adjust_heap_edge_count<T>(
    ptr: *mut Vec<T>,
    removed: usize,
    added: usize,
) -> usize {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    let current = unsafe { (*owner).heap_edge_count.load(Ordering::Relaxed) };
    let next = current
        .checked_sub(removed)
        .and_then(|count| count.checked_add(added))
        .unwrap_or_else(|| {
            eprintln!("molt fatal: list heap-edge count underflow or overflow");
            std::process::abort();
        });
    unsafe { (*owner).heap_edge_count.store(next, Ordering::Release) };
    next
}

/// Swap only the vector contents and their charged buffer ownership while
/// preserving each allocation's stable lock/generation identity.
///
/// # Safety
/// Both pointers must be distinct, live tracked Vec allocations of the same
/// element type. The caller must hold the live allocation's mutation lock and
/// exclusive custody of the staged allocation.
pub(crate) unsafe fn tracked_vec_swap_contents<T>(live: *mut Vec<T>, staged: *mut Vec<T>) {
    debug_assert_ne!(live, staged);
    let live_owner = unsafe { owner_ptr_from_vec(live) };
    let staged_owner = unsafe { owner_ptr_from_vec(staged) };
    unsafe {
        std::mem::swap(
            &mut (*live_owner).charge.buffer_bytes,
            &mut (*staged_owner).charge.buffer_bytes,
        );
        let live_edges = (*live_owner).heap_edge_count.load(Ordering::Relaxed);
        let staged_edges = (*staged_owner).heap_edge_count.load(Ordering::Relaxed);
        (*live_owner)
            .heap_edge_count
            .store(staged_edges, Ordering::Relaxed);
        (*staged_owner)
            .heap_edge_count
            .store(live_edges, Ordering::Relaxed);
        std::mem::swap(&mut (*live_owner).vec, &mut (*staged_owner).vec);
    }
}

/// Detach all elements and capacity from a live tracked Vec without allocating
/// a replacement owner. The caller must hold the stable mutation lock.
///
/// # Safety
/// `ptr` must be a live tracked Vec pointer under exclusive mutation custody.
pub(crate) unsafe fn tracked_vec_take_contents<T>(ptr: *mut Vec<T>) -> TrackedVecContents<T> {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    let vec = unsafe { std::mem::take(&mut (*owner).vec) };
    let buffer_bytes = unsafe { std::mem::take(&mut (*owner).charge.buffer_bytes) };
    unsafe { (*owner).heap_edge_count.store(0, Ordering::Release) };
    TrackedVecContents { vec, buffer_bytes }
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
/// Growth uses a replacement buffer that is fully charged before it becomes
/// visible through the stored Vec pointer. This keeps the tracker aligned with
/// actual allocator capacity, including allocator overgrant, and prevents a
/// failed over-limit reserve from leaving a larger buffer attached to the
/// object.
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
    let old_capacity = vec.capacity();
    let target_capacity = amortized_target_capacity(old_capacity, required_len);
    let Some(old_bytes) = vec_buffer_bytes::<T>(old_capacity) else {
        return false;
    };
    let Some(target_bytes) = vec_buffer_bytes::<T>(target_capacity) else {
        return false;
    };
    if !charge_grow(target_bytes) {
        return false;
    }

    let mut replacement = Vec::new();
    if replacement.try_reserve_exact(target_capacity).is_err() {
        release_grow(target_bytes);
        return false;
    }

    let Some(actual_bytes) = vec_buffer_bytes::<T>(replacement.capacity()) else {
        release_grow(target_bytes);
        return false;
    };
    if actual_bytes > target_bytes {
        let extra = actual_bytes - target_bytes;
        if !charge_grow(extra) {
            drop(replacement);
            release_grow(target_bytes);
            return false;
        }
    } else if actual_bytes < target_bytes {
        release_grow(target_bytes - actual_bytes);
    }

    replacement.append(vec);
    *vec = replacement;
    release_grow(old_bytes);
    unsafe {
        (*owner).charge.buffer_bytes = actual_bytes;
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

    #[test]
    fn tracked_vec_denied_growth_keeps_original_buffer() {
        let initial_charge = vec_charge::<u64>(4).expect("initial charge");
        let replacement_bytes = vec_buffer_bytes::<u64>(16).expect("replacement bytes");
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(initial_charge + replacement_bytes - 1),
            ..Default::default()
        })));
        let _reset = TrackerReset;
        let ptr = tracked_vec_box_with_capacity::<u64>(4).expect("tracked vec");
        unsafe {
            (*ptr).extend_from_slice(&[1, 2, 3, 4]);
            let original_data = (*ptr).as_ptr();
            assert!(!tracked_vec_reserve_for_len(ptr, 16));
            assert_eq!((*ptr).capacity(), 4);
            assert_eq!((*ptr).as_ptr(), original_data);
            assert_eq!((*ptr).as_slice(), &[1, 2, 3, 4]);
            drop(tracked_vec_box_from_raw(ptr));
        }
    }

    #[test]
    fn tracked_vec_growth_is_amortized() {
        set_tracker(Box::new(UnlimitedTracker));
        let _reset = TrackerReset;
        let ptr = tracked_vec_box_with_capacity::<u64>(4).expect("tracked vec");
        unsafe {
            (*ptr).extend_from_slice(&[1, 2, 3, 4]);
            let original_data = (*ptr).as_ptr();
            assert!(tracked_vec_reserve_for_len(ptr, 5));
            assert_eq!((*ptr).capacity(), 8);
            assert_eq!((*ptr).as_slice(), &[1, 2, 3, 4]);

            let grown_data = (*ptr).as_ptr();
            assert_ne!(grown_data, original_data);
            assert!(tracked_vec_reserve_for_len(ptr, 6));
            assert_eq!((*ptr).capacity(), 8);
            assert_eq!((*ptr).as_ptr(), grown_data);
            drop(tracked_vec_box_from_raw(ptr));
        }
    }
}
