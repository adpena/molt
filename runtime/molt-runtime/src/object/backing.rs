use super::molt_vec::MoltVec;
use owner_words::{EdgeCount, Generation, MutationLock};
use std::alloc::{Layout, alloc};
use std::mem::size_of;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};

/// A tracked Vec owner's mutation words: its lock, its publication generation
/// and its heap-edge count. A GIL build touches them only under the GIL, so
/// they are plain cells there, with no atomic read-modify-write on any list
/// read, append or store; a free-threaded build makes them atomics, as
/// `RefCountWord` is (molt-codegen-abi). The GIL lock keeps its state, so a
/// reentrant acquire aborts where it would otherwise spin forever.
#[cfg(not(feature = "free-threaded"))]
mod owner_words {
    use std::cell::Cell;

    pub(super) struct MutationLock(Cell<bool>);

    impl MutationLock {
        pub(super) const fn new() -> Self {
            Self(Cell::new(false))
        }

        #[inline(always)]
        pub(super) fn acquire(&self) {
            if self.0.replace(true) {
                eprintln!("molt fatal: reentrant sequence mutation lock");
                std::process::abort();
            }
        }

        #[inline(always)]
        pub(super) fn release(&self) {
            self.0.set(false);
        }

        #[cfg(test)]
        pub(super) fn is_locked(&self) -> bool {
            self.0.get()
        }
    }

    pub(super) struct Generation(Cell<u64>);

    impl Generation {
        pub(super) const fn new() -> Self {
            Self(Cell::new(0))
        }

        #[inline(always)]
        pub(super) fn get(&self) -> u64 {
            self.0.get()
        }

        #[inline(always)]
        pub(super) fn bump(&self) -> u64 {
            let next = self.0.get().wrapping_add(1);
            self.0.set(next);
            next
        }
    }

    pub(super) struct EdgeCount(Cell<usize>);

    impl EdgeCount {
        pub(super) const fn new() -> Self {
            Self(Cell::new(0))
        }

        #[inline(always)]
        pub(super) fn get(&self) -> usize {
            self.0.get()
        }

        #[inline(always)]
        pub(super) fn set(&self, count: usize) {
            self.0.set(count);
        }
    }
}

#[cfg(feature = "free-threaded")]
mod owner_words {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    pub(super) struct MutationLock(AtomicBool);

    impl MutationLock {
        pub(super) const fn new() -> Self {
            Self(AtomicBool::new(false))
        }

        #[inline]
        pub(super) fn acquire(&self) {
            while self
                .0
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                std::hint::spin_loop();
            }
        }

        #[inline]
        pub(super) fn release(&self) {
            self.0.store(false, Ordering::Release);
        }

        #[cfg(test)]
        pub(super) fn is_locked(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    pub(super) struct Generation(AtomicU64);

    impl Generation {
        pub(super) const fn new() -> Self {
            Self(AtomicU64::new(0))
        }

        #[inline]
        pub(super) fn get(&self) -> u64 {
            self.0.load(Ordering::Acquire)
        }

        #[inline]
        pub(super) fn bump(&self) -> u64 {
            self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
        }
    }

    pub(super) struct EdgeCount(AtomicUsize);

    impl EdgeCount {
        pub(super) const fn new() -> Self {
            Self(AtomicUsize::new(0))
        }

        #[inline]
        pub(super) fn get(&self) -> usize {
            self.0.load(Ordering::Acquire)
        }

        #[inline]
        pub(super) fn set(&self, count: usize) {
            self.0.store(count, Ordering::Release);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BackingCharge {
    owner_bytes: usize,
    buffer_bytes: usize,
}

#[repr(C)]
struct TrackedVecBox<T> {
    charge: BackingCharge,
    /// Monotonic publication generation for object-owned mutable sequence
    /// storage: the observation seam for callback reentrancy.
    mutation_epoch: Generation,
    mutation_lock: MutationLock,
    /// Exact heap-reference edge count for generic list/tuple storage. Other
    /// tracked Vec owners leave this at zero. Keeping it beside the stable
    /// mutation generation removes full-sequence rescans from scalar mutation
    /// hot paths.
    heap_edge_count: EdgeCount,
    buffer_exports: super::buffer_exports::BufferExports,
    vec: MoltVec<T>,
}

pub(crate) struct TrackedVecMutationGuard<T> {
    owner: NonNull<TrackedVecBox<T>>,
}

impl<T> Drop for TrackedVecMutationGuard<T> {
    fn drop(&mut self) {
        unsafe { self.owner.as_ref().mutation_lock.release() }
    }
}

pub(crate) struct TrackedVecOwner<T> {
    ptr: NonNull<TrackedVecBox<T>>,
}

impl<T> TrackedVecOwner<T> {
    /// Transfer this allocation, including its accounting and mutation identity,
    /// to an object that assumes sole responsibility for terminal destruction.
    pub(crate) fn into_raw(self) -> *mut MoltVec<T> {
        let owner = std::mem::ManuallyDrop::new(self);
        unsafe { &raw mut (*owner.ptr.as_ptr()).vec }
    }
}

/// Detached backing-buffer custody from a still-live tracked Vec owner. The
/// stable owner retains its lock/epoch identity with an empty zero-charge Vec;
/// dropping this value releases the displaced buffer's resource charge.
pub(crate) struct TrackedVecContents<T> {
    vec: MoltVec<T>,
    buffer_bytes: usize,
}

impl<T> Deref for TrackedVecOwner<T> {
    type Target = MoltVec<T>;

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
            if boxed.buffer_exports.is_exported() {
                eprintln!("molt fatal: destroying storage with live buffer exports");
                std::process::abort();
            }
            let charge = boxed.charge;
            drop(boxed);
            release_alloc(charge.owner_bytes.saturating_add(charge.buffer_bytes));
        }
    }
}

impl<T> Deref for TrackedVecContents<T> {
    type Target = MoltVec<T>;

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
        // Owner and initial buffer share one allocation charge. Detaching the
        // buffer transfers bytes, not the owner's allocation-count obligation.
        release_grow(self.buffer_bytes);
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
unsafe fn owner_ptr_from_vec<T>(ptr: *mut MoltVec<T>) -> *mut TrackedVecBox<T> {
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
    crate::resource::charge_allocation(bytes).is_ok()
}

#[inline]
pub(crate) fn charge_grow(bytes: usize) -> bool {
    if bytes == 0 {
        return true;
    }
    crate::resource::charge_growth(bytes).is_ok()
}

#[inline]
pub(crate) fn release_alloc(bytes: usize) {
    if bytes == 0 {
        return;
    }
    crate::resource::release_allocation(bytes);
}

#[inline]
pub(crate) fn release_grow(bytes: usize) {
    if bytes == 0 {
        return;
    }
    crate::resource::release_growth(bytes);
}

/// A pre-sized buffer whose owner can live inside an object. Unlike a tracked
/// Vec box, it needs no separate header allocation, export state, or lock.
/// Pushes initialize reserved cells and can never grow the allocation.
#[repr(transparent)]
pub(crate) struct FixedBacking<T>(MoltVec<T>);

impl<T> FixedBacking<T> {
    pub(crate) fn with_capacity(capacity: usize) -> Option<Self> {
        let layout = Layout::array::<T>(capacity).ok()?;
        if layout.size() == 0 {
            return Some(Self(MoltVec::from_vec(Vec::new())));
        }
        if !charge_alloc(layout.size()) {
            return None;
        }
        // Allocate this exact array so its retained charge is recoverable from
        // the capacity alone. The vector owns only the initialized prefix.
        let pointer = unsafe { alloc(layout).cast::<T>() };
        if pointer.is_null() {
            release_alloc(layout.size());
            return None;
        }
        let values = unsafe { Vec::from_raw_parts(pointer, 0, capacity) };
        Some(Self(MoltVec::from_vec(values)))
    }

    pub(crate) fn push(&mut self, value: T) {
        assert!(self.len() < self.capacity());
        self.0.push(value);
    }

    pub(crate) fn resize(&mut self, len: usize, value: T)
    where
        T: Clone,
    {
        assert!(len <= self.capacity());
        self.0.resize(len, value);
    }

    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

impl<T> Deref for FixedBacking<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.0.as_slice()
    }
}

impl<T> DerefMut for FixedBacking<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.0.as_mut_slice()
    }
}

impl<T> Drop for FixedBacking<T> {
    fn drop(&mut self) {
        release_alloc(self.0.capacity() * size_of::<T>());
    }
}

/// Allocate an object-owned Vec with resource accounting for both the owning
/// allocation and its backing buffer capacity.
pub(crate) fn tracked_vec_box_with_capacity<T>(capacity: usize) -> Option<*mut MoltVec<T>> {
    tracked_vec_box_with_buffer(capacity, |capacity| {
        let mut vec = Vec::new();
        (capacity == 0 || vec.try_reserve_exact(capacity).is_ok()).then_some(vec)
    })
}

/// Element types whose all-zero bit pattern is their `Default` value.
///
/// # Safety
/// An implementation asserts exactly that, for every value of the type.
pub(crate) unsafe trait ZeroIsDefault: Copy + Default {}
unsafe impl ZeroIsDefault for usize {}
unsafe impl ZeroIsDefault for u64 {}
unsafe impl ZeroIsDefault for i64 {}
unsafe impl ZeroIsDefault for u8 {}

/// Tracked storage of `len` default elements from one zeroed allocation: a
/// fresh hash table, which a fill loop would otherwise write element by element.
pub(crate) fn tracked_vec_box_zeroed<T: ZeroIsDefault>(len: usize) -> Option<*mut MoltVec<T>> {
    tracked_vec_box_with_buffer(len, |len| {
        if len == 0 || size_of::<T>() == 0 {
            return Some(Vec::new());
        }
        let layout = Layout::array::<T>(len).ok()?;
        let data = unsafe { std::alloc::alloc_zeroed(layout) }.cast::<T>();
        // Every element is the zero pattern, which ZeroIsDefault makes `T::default()`.
        (!data.is_null()).then(|| unsafe { Vec::from_raw_parts(data, len, len) })
    })
}

/// Charge for `capacity` elements, build the buffer with `make` (None when the
/// allocator refuses it), and box it with its accounting.
fn tracked_vec_box_with_buffer<T>(
    capacity: usize,
    make: impl FnOnce(usize) -> Option<Vec<T>>,
) -> Option<*mut MoltVec<T>> {
    let requested_charge = vec_charge::<T>(capacity)?;
    if !charge_alloc(requested_charge) {
        return None;
    }

    let Some(vec) = make(capacity) else {
        release_alloc(requested_charge);
        return None;
    };

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
                mutation_epoch: Generation::new(),
                mutation_lock: MutationLock::new(),
                heap_edge_count: EdgeCount::new(),
                buffer_exports: super::buffer_exports::BufferExports::new(),
                vec: MoltVec::from_vec(vec),
            },
        );
        Some(ptr::addr_of_mut!((*raw).vec))
    }
}

/// Acquire the stable mutation lock associated with an object-owned vector:
/// a flag under the GIL, the per-object publication boundary in a
/// free-threaded build (`owner_words`).
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
pub(crate) unsafe fn tracked_vec_mutation_lock<T>(
    ptr: *mut MoltVec<T>,
) -> TrackedVecMutationGuard<T> {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).mutation_lock.acquire() };
    TrackedVecMutationGuard {
        owner: unsafe { NonNull::new_unchecked(owner) },
    }
}

// Observe the real publication lock without waiting inside a native finalizer.
#[cfg(test)]
pub(crate) unsafe fn tracked_vec_mutation_is_locked_for_test<T>(ptr: *mut MoltVec<T>) -> bool {
    unsafe { (*owner_ptr_from_vec(ptr)).mutation_lock.is_locked() }
}

/// Return the current publication generation for tracked sequence storage.
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
#[inline]
pub(crate) unsafe fn tracked_vec_mutation_epoch<T>(ptr: *mut MoltVec<T>) -> u64 {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).mutation_epoch.get() }
}

/// Export state lives with the stable allocation, never in an address registry.
pub(crate) unsafe fn tracked_vec_buffer_exports<T>(
    ptr: *mut MoltVec<T>,
) -> &'static super::buffer_exports::BufferExports {
    unsafe { &(*owner_ptr_from_vec(ptr)).buffer_exports }
}

/// Publish one in-place mutation and return the new generation.
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
#[inline]
pub(crate) unsafe fn tracked_vec_bump_mutation_epoch<T>(ptr: *mut MoltVec<T>) -> u64 {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).mutation_epoch.bump() }
}

#[inline]
pub(crate) unsafe fn tracked_vec_heap_edge_count<T>(ptr: *mut MoltVec<T>) -> usize {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).heap_edge_count.get() }
}

#[inline]
pub(crate) unsafe fn tracked_vec_set_heap_edge_count<T>(ptr: *mut MoltVec<T>, count: usize) {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    unsafe { (*owner).heap_edge_count.set(count) };
}

#[inline]
pub(crate) unsafe fn tracked_vec_adjust_heap_edge_count<T>(
    ptr: *mut MoltVec<T>,
    removed: usize,
    added: usize,
) -> usize {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    let current = unsafe { (*owner).heap_edge_count.get() };
    let next = current
        .checked_sub(removed)
        .and_then(|count| count.checked_add(added))
        .unwrap_or_else(|| {
            eprintln!("molt fatal: list heap-edge count underflow or overflow");
            std::process::abort();
        });
    unsafe { (*owner).heap_edge_count.set(next) };
    next
}

/// Swap only the vector contents and their charged buffer ownership while
/// preserving each allocation's stable lock/generation identity.
///
/// # Safety
/// Both pointers must be distinct, live tracked Vec allocations of the same
/// element type. The caller must hold the live allocation's mutation lock and
/// exclusive custody of the staged allocation.
pub(crate) unsafe fn tracked_vec_swap_contents<T>(live: *mut MoltVec<T>, staged: *mut MoltVec<T>) {
    debug_assert_ne!(live, staged);
    let live_owner = unsafe { owner_ptr_from_vec(live) };
    let staged_owner = unsafe { owner_ptr_from_vec(staged) };
    unsafe {
        assert!(!(*live_owner).buffer_exports.is_exported());
        assert!(!(*staged_owner).buffer_exports.is_exported());
        std::mem::swap(
            &mut (*live_owner).charge.buffer_bytes,
            &mut (*staged_owner).charge.buffer_bytes,
        );
        let live_edges = (*live_owner).heap_edge_count.get();
        let staged_edges = (*staged_owner).heap_edge_count.get();
        (*live_owner).heap_edge_count.set(staged_edges);
        (*staged_owner).heap_edge_count.set(live_edges);
        std::mem::swap(&mut (*live_owner).vec, &mut (*staged_owner).vec);
    }
}

/// Detach all elements and capacity from a live tracked Vec without allocating
/// a replacement owner. The caller must hold the stable mutation lock.
///
/// # Safety
/// `ptr` must be a live tracked Vec pointer under exclusive mutation custody.
pub(crate) unsafe fn tracked_vec_take_contents<T>(ptr: *mut MoltVec<T>) -> TrackedVecContents<T> {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    assert!(!unsafe { (*owner).buffer_exports.is_exported() });
    let vec = unsafe { std::mem::take(&mut (*owner).vec) };
    let buffer_bytes = unsafe { std::mem::take(&mut (*owner).charge.buffer_bytes) };
    unsafe { (*owner).heap_edge_count.set(0) };
    TrackedVecContents { vec, buffer_bytes }
}

pub(crate) fn tracked_vec_box_from_slice<T: Copy>(
    elems: &[T],
    capacity: usize,
) -> Option<*mut MoltVec<T>> {
    let ptr = tracked_vec_box_with_capacity::<T>(capacity.max(elems.len()))?;
    unsafe {
        (*ptr).extend_from_slice(elems);
    }
    Some(ptr)
}

/// Tracked storage of `len` copies of `value`.
pub(crate) fn tracked_vec_box_filled<T: Copy>(len: usize, value: T) -> Option<*mut MoltVec<T>> {
    let ptr = tracked_vec_box_with_capacity::<T>(len)?;
    unsafe {
        (*ptr).resize(len, value);
    }
    Some(ptr)
}

/// Tracked storage of `items` repeated `times` times.
pub(crate) fn tracked_vec_box_repeated<T: Copy>(
    items: &[T],
    times: usize,
) -> Option<*mut MoltVec<T>> {
    let ptr = tracked_vec_box_with_capacity::<T>(items.len().checked_mul(times)?)?;
    for _ in 0..times {
        unsafe { (*ptr).extend_from_slice(items) };
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
/// object. This storage phase never selects a Python error or retires Python
/// owners. Callers must release mutation guards and borrowed storage before
/// selecting the allocation failure, because that can run displaced finalizers.
///
/// # Safety
/// `ptr` must be a live Vec pointer allocated by
/// [`tracked_vec_box_with_capacity`].
#[inline]
pub(crate) unsafe fn tracked_vec_reserve_for_len<T>(
    ptr: *mut MoltVec<T>,
    required_len: usize,
) -> bool {
    if ptr.is_null() {
        return false;
    }
    if required_len <= unsafe { (*ptr).capacity() } {
        return true;
    }
    unsafe { tracked_vec_grow_for_len(ptr, required_len) }
}

/// The charged regrowth behind [`tracked_vec_reserve_for_len`], out of line:
/// inlined, it spilled the registers of every caller's common no-growth path
/// (`set_insert_absent_with_hash` grew by 46 instructions, 2026-11-14).
#[cold]
#[inline(never)]
unsafe fn tracked_vec_grow_for_len<T>(ptr: *mut MoltVec<T>, required_len: usize) -> bool {
    let owner = unsafe { owner_ptr_from_vec(ptr) };
    let vec = unsafe { &mut *ptr };
    if unsafe { (*owner).buffer_exports.is_exported() } {
        return false;
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

    let mut old = std::mem::take(vec).into_vec();
    replacement.append(&mut old);
    drop(old);
    *vec = MoltVec::from_vec(replacement);
    release_grow(old_bytes);
    unsafe {
        (*owner).charge.buffer_bytes = actual_bytes;
    }
    true
}

/// Reconstruct the owning tracked Vec allocation from the stored Vec pointer.
///
/// # Safety
/// `ptr` must be a pointer returned by [`tracked_vec_box_with_capacity`]. The
/// returned owner must be dropped exactly once by the caller.
pub(crate) unsafe fn tracked_vec_box_from_raw<T>(ptr: *mut MoltVec<T>) -> TrackedVecOwner<T> {
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

    #[test]
    fn tracked_vec_capacity_is_charged_and_released() {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(vec_charge::<u64>(8).unwrap()),
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
    fn detached_contents_release_bytes_without_stealing_live_allocation_counts() {
        for owner_first in [false, true] {
            for capacity in [0, 4, 16] {
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(2),
                    max_memory: Some(
                        vec_charge::<u64>(0).unwrap() + vec_charge::<u64>(capacity).unwrap(),
                    ),
                    ..Default::default()
                })));
                let _reset = crate::test_support::restore_unlimited_tracker_on_drop();
                let sentinel = tracked_vec_box_with_capacity::<u64>(0).unwrap();
                let ptr = tracked_vec_box_with_capacity::<u64>(capacity).unwrap();
                let contents = unsafe { tracked_vec_take_contents(ptr) };
                if owner_first {
                    unsafe { drop(tracked_vec_box_from_raw(ptr)) };
                    drop(contents);
                } else {
                    drop(contents);
                    assert!(
                        with_tracker(|tracker| tracker.on_allocate(0)).is_err(),
                        "detached buffer must not decrement its live owner's count"
                    );
                    unsafe { drop(tracked_vec_box_from_raw(ptr)) };
                }
                // The sentinel remains live: exactly one count and the complete
                // displaced owner's byte budget must now be reusable.
                let replacement = tracked_vec_box_with_capacity::<u64>(capacity).unwrap();
                assert!(
                    with_tracker(|tracker| tracker.on_allocate(0)).is_err(),
                    "rollback stole the unrelated sentinel's allocation count"
                );
                unsafe {
                    drop(tracked_vec_box_from_raw(replacement));
                    drop(tracked_vec_box_from_raw(sentinel));
                }
            }
        }
    }

    #[test]
    fn tracked_vec_denied_growth_keeps_original_buffer() {
        let initial_charge = vec_charge::<u64>(4).expect("initial charge");
        let replacement_bytes = vec_buffer_bytes::<u64>(16).expect("replacement bytes");
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(initial_charge + replacement_bytes - 1),
            ..Default::default()
        })));
        let _reset = crate::test_support::restore_unlimited_tracker_on_drop();
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
        let _reset = crate::test_support::restore_unlimited_tracker_on_drop();
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
