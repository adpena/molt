//! Per-function nursery for short-lived objects.
//! Bump allocation: ~2 instructions (compare + pointer increment).
//! Reset: 1 instruction (reset cursor to base).
//!
//! SAFETY: Objects allocated in the nursery MUST NOT outlive the nursery.
//! The write barrier detects when a nursery pointer is stored into a
//! heap-resident container and promotes the nursery object to the heap.

const NURSERY_SIZE: usize = 64 * 1024; // 64KB
const NURSERY_WORDS: usize = NURSERY_SIZE / std::mem::size_of::<u64>();

pub struct Nursery {
    data: Vec<u64>, // 8-byte-aligned backing storage (heap-allocated, reusable)
    cursor: usize,  // next allocation offset
}

impl Nursery {
    pub fn new() -> Self {
        Self {
            data: vec![0u64; NURSERY_WORDS],
            cursor: 0,
        }
    }

    /// Create a nursery with zero-capacity backing storage.
    /// Used during shutdown to replace the active nursery so that when
    /// Rust's TLS destructor runs, `Vec::drop` is a no-op (no dealloc).
    pub fn empty() -> Self {
        Self {
            data: Vec::new(),
            cursor: 0,
        }
    }

    /// Bump-allocate `size` bytes with `align` alignment.
    /// Returns None if nursery is full (caller falls back to heap).
    ///
    /// # Panics
    /// Debug-asserts that `align` is a power of two and `size > 0`.
    #[inline(always)]
    pub fn alloc(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        debug_assert!(
            align.is_power_of_two(),
            "alignment must be a power of two, got {align}"
        );
        debug_assert!(size > 0, "zero-size allocations are not supported");
        // Guard against align=0 in release builds (would cause !(align-1) = !usize::MAX = 0,
        // making aligned = 0 regardless of cursor — incorrect but not UB).
        if align == 0 || size == 0 {
            return None;
        }
        let aligned = (self.cursor + align - 1) & !(align - 1);
        let new_cursor = aligned + size;
        if new_cursor <= NURSERY_SIZE {
            let ptr = unsafe { (self.data.as_mut_ptr() as *mut u8).add(aligned) };
            self.cursor = new_cursor;
            Some(ptr)
        } else {
            None
        }
    }

    /// Check if a pointer belongs to this nursery.
    #[inline(always)]
    pub fn contains(&self, ptr: *const u8) -> bool {
        let base = self.data.as_ptr() as usize;
        let addr = ptr as usize;
        addr >= base && addr < base + NURSERY_SIZE
    }

    /// Reset the nursery. All nursery-allocated objects become invalid.
    /// Call at function exit after all nursery objects are dead.
    #[inline(always)]
    pub fn reset(&mut self) {
        self.cursor = 0;
        // Note: we don't zero the memory — next alloc will overwrite
    }

    /// Write barrier: call this when storing `value` into a field of `target`.
    /// If `value` is a nursery pointer and `target` is NOT in the nursery,
    /// the value must be promoted to the heap before the store.
    ///
    /// Returns the (possibly promoted) value pointer to actually store.
    #[inline(always)]
    pub fn write_barrier(&self, target: *const u8, value: *mut u8, size: usize) -> *mut u8 {
        if self.contains(value) && !self.contains(target) {
            // Promote: copy to heap
            Self::promote(value, size)
        } else {
            value // No promotion needed
        }
    }

    /// Copy a nursery object to the heap.
    ///
    /// # Safety
    /// Returns null if heap allocation fails (OOM). Caller must handle null.
    fn promote(nursery_ptr: *mut u8, size: usize) -> *mut u8 {
        if size == 0 {
            return std::ptr::null_mut();
        }
        unsafe {
            let layout = std::alloc::Layout::from_size_align(size, 8)
                .expect("invalid layout in nursery promote");
            let heap_ptr = std::alloc::alloc(layout);
            if heap_ptr.is_null() {
                // OOM: call the global handler rather than UB from null deref
                std::alloc::handle_alloc_error(layout);
            }
            std::ptr::copy_nonoverlapping(nursery_ptr, heap_ptr, size);
            heap_ptr
        }
    }

    /// How many bytes are currently allocated.
    pub fn used(&self) -> usize {
        self.cursor
    }

    /// How many bytes remain available.
    pub fn remaining(&self) -> usize {
        NURSERY_SIZE - self.cursor
    }
}

impl Default for Nursery {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_nursery_with_zero_used() {
        let n = Nursery::new();
        assert_eq!(n.used(), 0);
        assert_eq!(n.remaining(), NURSERY_SIZE);
    }

    #[test]
    fn alloc_returns_valid_pointer_and_advances_cursor() {
        let mut n = Nursery::new();
        let ptr = n.alloc(16, 1);
        assert!(ptr.is_some());
        assert_eq!(n.used(), 16);
    }

    #[test]
    fn multiple_allocs_are_contiguous() {
        let mut n = Nursery::new();
        let p1 = n.alloc(16, 1).unwrap();
        let p2 = n.alloc(16, 1).unwrap();
        // p2 should be exactly 16 bytes after p1
        assert_eq!(p2 as usize, p1 as usize + 16);
    }

    #[test]
    fn alloc_with_alignment_works() {
        let mut n = Nursery::new();
        // Misalign the cursor first
        let _ = n.alloc(3, 1).unwrap(); // cursor now at 3
        let p = n.alloc(8, 8).unwrap();
        // Returned pointer must be 8-byte aligned
        assert_eq!(p as usize % 8, 0);
    }

    #[test]
    fn nursery_base_is_8_byte_aligned() {
        let n = Nursery::new();
        assert_eq!(n.data.as_ptr() as usize % 8, 0);
    }

    #[test]
    fn alloc_returns_none_when_nursery_is_full() {
        let mut n = Nursery::new();
        // Fill the nursery completely
        let first = n.alloc(NURSERY_SIZE, 1);
        assert!(first.is_some());
        // Next alloc should fail
        let overflow = n.alloc(1, 1);
        assert!(overflow.is_none());
    }

    #[test]
    fn contains_returns_true_for_nursery_pointer_false_for_heap() {
        let mut n = Nursery::new();
        let ptr = n.alloc(8, 1).unwrap();
        assert!(n.contains(ptr));

        // A stack/heap address outside the nursery
        let stack_val: u8 = 42;
        assert!(!n.contains(&stack_val as *const u8));
    }

    #[test]
    fn reset_resets_cursor_to_zero() {
        let mut n = Nursery::new();
        let _ = n.alloc(100, 1).unwrap();
        assert_eq!(n.used(), 100);
        n.reset();
        assert_eq!(n.used(), 0);
        assert_eq!(n.remaining(), NURSERY_SIZE);
    }

    #[test]
    fn after_reset_new_allocs_reuse_same_memory_region() {
        let mut n = Nursery::new();
        let p1 = n.alloc(16, 1).unwrap();
        n.reset();
        let p2 = n.alloc(16, 1).unwrap();
        // After reset, allocation starts from the same base offset
        assert_eq!(p1, p2);
    }

    #[test]
    fn write_barrier_returns_same_pointer_when_both_in_nursery() {
        let mut n = Nursery::new();
        let target = n.alloc(32, 8).unwrap();
        let value = n.alloc(16, 8).unwrap();
        let result = n.write_barrier(target, value, 16);
        // Both are in nursery — no promotion, same pointer returned
        assert_eq!(result, value);
    }

    #[test]
    fn write_barrier_promotes_nursery_ptr_into_heap_object() {
        let mut n = Nursery::new();
        let value = n.alloc(16, 8).unwrap();
        // Write a known pattern so we can verify copy
        unsafe { std::ptr::write_bytes(value, 0xAB, 16) };

        // target is a heap address (outside the nursery)
        let heap_target = Box::new([0u8; 32]);
        let target_ptr = heap_target.as_ptr();

        let result = n.write_barrier(target_ptr, value, 16);
        // Promotion must have produced a *different* pointer (heap copy)
        assert_ne!(result, value);
        assert!(!n.contains(result));
        // Content must match
        let src_slice = unsafe { std::slice::from_raw_parts(value, 16) };
        let dst_slice = unsafe { std::slice::from_raw_parts(result, 16) };
        assert_eq!(src_slice, dst_slice);

        // Clean up promoted allocation
        unsafe {
            std::alloc::dealloc(result, std::alloc::Layout::from_size_align(16, 8).unwrap());
        }
    }
}
