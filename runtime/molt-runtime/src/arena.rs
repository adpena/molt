use std::alloc::Layout;
use std::mem::{align_of, size_of};
use std::sync::atomic::Ordering as AtomicOrdering;

use crate::object::{HEADER_FLAG_ARENA, HEADER_FLAG_RAW_ALLOC};
use crate::{MoltHeader, MoltObject, TYPE_ID_OBJECT, usize_from_bits};

pub struct TempArena {
    chunk_size: usize,
    chunks: Vec<Vec<u8>>,
    offset: usize,
}

impl TempArena {
    pub fn new(chunk_size: usize) -> Self {
        let size = chunk_size.max(1024);
        Self {
            chunk_size: size,
            chunks: vec![vec![0u8; size]],
            offset: 0,
        }
    }

    pub fn reset(&mut self) {
        if self.chunks.is_empty() {
            self.chunks.push(vec![0u8; self.chunk_size]);
        } else {
            self.chunks.truncate(1);
        }
        self.offset = 0;
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.offset = 0;
    }

    /// Release ALL heap memory, including the outer Vec's buffer.
    /// After this call, dropping `self` will not invoke the allocator.
    pub fn drain(&mut self) {
        self.chunks = Vec::new();
        self.offset = 0;
    }

    pub fn alloc_slice<T>(&mut self, len: usize) -> *mut T {
        if len == 0 {
            return std::ptr::null_mut();
        }
        let align = align_of::<T>();
        let size = match len.checked_mul(size_of::<T>()) {
            Some(val) => val,
            None => return std::ptr::null_mut(),
        };
        let aligned = (self.offset + (align - 1)) & !(align - 1);
        let needed = aligned.saturating_add(size);
        if needed > self.chunks.last().unwrap().len() {
            let new_size = self.chunk_size.max(size);
            self.chunks.push(vec![0u8; new_size]);
            self.offset = 0;
            return self.alloc_slice::<T>(len);
        }
        let ptr = unsafe { self.chunks.last_mut().unwrap().as_mut_ptr().add(aligned) };
        self.offset = needed;
        ptr as *mut T
    }
}

// ---------------------------------------------------------------------------
// ScopeArena — per-scope bump allocator for NoEscape values
// ---------------------------------------------------------------------------
//
// MLKit/Cyclone-style region allocator. The compiler emits arena lifecycle
// calls at scope boundaries: create at scope entry, bump-allocate NoEscape
// values during scope execution, reset/free at scope exit. All allocations
// within a scope are freed in O(1) by resetting the bump pointer.

const SCOPE_ARENA_CHUNK_SIZE: usize = 4096;
const SCOPE_ARENA_ALIGN: usize = 8;

/// Owned arena chunk with guaranteed `SCOPE_ARENA_ALIGN`-byte alignment.
///
/// Backed by `std::alloc::alloc_zeroed` rather than `Vec<u8>` because the
/// Vec allocator only guarantees `align_of::<u8>() == 1`, while
/// `MoltHeader` requires ≥ 4-byte alignment for its `u32`/`AtomicU32`
/// fields and the bump allocator promises 8-byte aligned hand-outs.
///
/// `Drop` releases the chunk via `std::alloc::dealloc` with the same
/// `Layout` used to allocate it.
struct ArenaChunk {
    ptr: *mut u8,
    capacity: usize,
}

impl ArenaChunk {
    fn new(capacity: usize) -> Self {
        // Layout panics on overflow / zero alignment; both are precluded
        // here: `capacity > 0` (callers pass `>= SCOPE_ARENA_CHUNK_SIZE`)
        // and `SCOPE_ARENA_ALIGN` is a non-zero power of two.
        let layout = Layout::from_size_align(capacity, SCOPE_ARENA_ALIGN)
            .expect("arena chunk layout must be valid");
        // SAFETY: `layout.size() > 0`.  Returns a non-null, aligned,
        // zeroed pointer or `handle_alloc_error` aborts.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Self { ptr, capacity }
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }
}

impl Drop for ArenaChunk {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.capacity, SCOPE_ARENA_ALIGN)
            .expect("arena chunk layout must be valid");
        // SAFETY: `ptr` was returned by `alloc_zeroed` with this exact
        // `layout` and has not been freed.
        unsafe {
            std::alloc::dealloc(self.ptr, layout);
        }
    }
}

// SAFETY: `ArenaChunk` owns its allocation and exposes no shared
// references; sending it across threads is sound.  `ScopeArena` itself
// is not Sync (it has interior mutation), but Send lets the arena be
// transferred to a worker thread for scope execution.
unsafe impl Send for ArenaChunk {}

/// Per-scope bump allocator for NoEscape values.
///
/// All allocations are `SCOPE_ARENA_ALIGN` (8) byte aligned. At scope
/// exit the entire arena is freed in O(1) by resetting the bump pointer
/// (or dropping the arena). Chunks are allocated on demand and reused
/// across resets.
pub struct ScopeArena {
    /// Backing storage. Each entry is an aligned heap-allocated chunk.
    chunks: Vec<ArenaChunk>,
    /// Next free byte in the current (last) chunk.
    current: *mut u8,
    /// Bytes remaining in the current chunk.
    remaining: usize,
}

impl ScopeArena {
    pub fn new() -> Self {
        let mut chunk = ArenaChunk::new(SCOPE_ARENA_CHUNK_SIZE);
        let ptr = chunk.as_mut_ptr();
        Self {
            chunks: vec![chunk],
            current: ptr,
            remaining: SCOPE_ARENA_CHUNK_SIZE,
        }
    }

    /// Bump-allocate `size` bytes with `SCOPE_ARENA_ALIGN`-byte alignment.
    ///
    /// Returns a null pointer only if `size` is zero.
    #[inline]
    pub fn alloc(&mut self, size: usize) -> *mut u8 {
        if size == 0 {
            return std::ptr::null_mut();
        }
        let aligned_size = (size + SCOPE_ARENA_ALIGN - 1) & !(SCOPE_ARENA_ALIGN - 1);
        if aligned_size <= self.remaining {
            let ptr = self.current;
            // SAFETY: `current` points into the active chunk's allocation
            // and `aligned_size <= remaining` keeps us inside it.
            self.current = unsafe { self.current.add(aligned_size) };
            self.remaining -= aligned_size;
            ptr
        } else {
            self.alloc_slow(aligned_size)
        }
    }

    #[cold]
    fn alloc_slow(&mut self, aligned_size: usize) -> *mut u8 {
        let chunk_cap = aligned_size.max(SCOPE_ARENA_CHUNK_SIZE);
        let mut chunk = ArenaChunk::new(chunk_cap);
        let ptr = chunk.as_mut_ptr();
        // SAFETY: `aligned_size <= chunk_cap` by construction above.
        self.current = unsafe { ptr.add(aligned_size) };
        self.remaining = chunk_cap - aligned_size;
        self.chunks.push(chunk);
        ptr
    }

    /// Reset the arena -- frees ALL allocations in O(1).
    ///
    /// Keeps the first chunk allocated so the next scope entry avoids a
    /// fresh allocation for the common case.
    pub fn reset(&mut self) {
        self.chunks.truncate(1);
        if let Some(first) = self.chunks.first_mut() {
            self.current = first.as_mut_ptr();
            self.remaining = SCOPE_ARENA_CHUNK_SIZE;
        }
    }
}

// ---------------------------------------------------------------------------
// C ABI exports for compiler-emitted scope arena lifecycle
// ---------------------------------------------------------------------------

/// Create a new scope arena. Returns a heap-allocated pointer.
/// The caller must pair this with `molt_arena_free`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_arena_new() -> *mut ScopeArena {
    Box::into_raw(Box::new(ScopeArena::new()))
}

/// Bump-allocate `size` bytes from the arena.
/// Returns a null pointer if `size` is zero or `arena` is null.
#[unsafe(no_mangle)]
pub extern "C" fn molt_arena_alloc(arena: *mut ScopeArena, size: u64) -> *mut u8 {
    if arena.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees `arena` was returned by `molt_arena_new`
    // and has not been freed.
    let arena = unsafe { &mut *arena };
    arena.alloc(size as usize)
}

/// Bump-allocate a `MoltObject` (header + payload) inside the arena and
/// return its NaN-boxed bits.
///
/// Mirrors the contract of [`molt_alloc`] (see `object/builders.rs`):
/// `size_bits` is the requested **payload** size in bytes; the header is
/// added on top. The returned `u64` is `MoltObject::from_ptr(obj_ptr).bits()`,
/// not a raw pointer — every consumer of the result (field stores, refcount
/// ops, etc.) expects fully NaN-boxed bits.
///
/// The header is initialized as `TYPE_ID_OBJECT` with refcount 1 and the
/// `HEADER_FLAG_ARENA | HEADER_FLAG_RAW_ALLOC` flags set so `dec_ref` skips
/// the global allocator (the arena reclaims memory via `molt_arena_free`).
///
/// On null `arena` or arena OOM, returns `MoltObject::none().bits()`, again
/// matching `molt_alloc`'s failure semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_arena_alloc_object(arena: *mut ScopeArena, size_bits: u64) -> u64 {
    if arena.is_null() {
        return MoltObject::none().bits();
    }
    crate::with_gil_entry_nopanic!(_py, {
        let payload = usize_from_bits(size_bits);
        let total = match payload.checked_add(size_of::<MoltHeader>()) {
            Some(v) => v,
            None => return MoltObject::none().bits(),
        };
        // SAFETY: caller guarantees `arena` was returned by `molt_arena_new`
        // and has not been freed.
        let arena_ref = unsafe { &mut *arena };
        let header_ptr = arena_ref.alloc(total);
        if header_ptr.is_null() {
            return MoltObject::none().bits();
        }
        // Zero header + payload so subsequent stores see a clean slate, just
        // like `alloc_object_zeroed_with_pool` does for pool-eligible types.
        // SAFETY: `arena.alloc` returned a chunk of `total` bytes belonging
        // to a live `Vec<u8>` inside the arena.
        unsafe {
            std::ptr::write_bytes(header_ptr, 0, total);
            let header = header_ptr as *mut MoltHeader;
            (*header).type_id = TYPE_ID_OBJECT;
            (*header).ref_count.store(1, AtomicOrdering::Relaxed);
            // size_class = 0 (oversized path) keeps drop logic generic — we
            // never feed arena allocations to the object pool, and the arena
            // free path bypasses `std::alloc::dealloc` entirely.
            (*header).size_class = 0;
            (*header).flags = HEADER_FLAG_ARENA | HEADER_FLAG_RAW_ALLOC;
            // cold_idx remains 0; arena objects are short-lived and never
            // need extended metadata.
            let obj_ptr = header_ptr.add(size_of::<MoltHeader>());
            MoltObject::from_ptr(obj_ptr).bits()
        }
    })
}

/// Reset the arena, releasing all bump allocations in O(1).
/// The arena itself remains valid for reuse.
#[unsafe(no_mangle)]
pub extern "C" fn molt_arena_reset(arena: *mut ScopeArena) {
    if arena.is_null() {
        return;
    }
    // SAFETY: caller guarantees `arena` was returned by `molt_arena_new`
    // and has not been freed.
    let arena = unsafe { &mut *arena };
    arena.reset();
}

/// Free the arena and all of its backing storage.
/// After this call, `arena` is dangling and must not be used.
#[unsafe(no_mangle)]
pub extern "C" fn molt_arena_free(arena: *mut ScopeArena) {
    if !arena.is_null() {
        // SAFETY: caller guarantees this was returned by `molt_arena_new`
        // and has not been freed yet.
        let _ = unsafe { Box::from_raw(arena) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_alloc_object_returns_nan_boxed_pointer() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let arena = molt_arena_new();
            let bits = molt_arena_alloc_object(arena, 16);
            // Recover the underlying pointer from the NaN-boxed bits.
            let obj = MoltObject::from_bits(bits);
            let ptr = obj.as_ptr().expect("expected non-null heap pointer");
            unsafe {
                let header = crate::object::header_from_obj_ptr(ptr);
                assert_eq!((*header).type_id, TYPE_ID_OBJECT);
                assert_eq!(
                    (*header).ref_count.load(AtomicOrdering::Relaxed),
                    1,
                    "fresh arena alloc should have refcount 1"
                );
                assert_ne!(
                    (*header).flags & HEADER_FLAG_ARENA,
                    0,
                    "HEADER_FLAG_ARENA must be set so dec_ref skips dealloc"
                );
            }
            molt_arena_free(arena);
        });
    }

    #[test]
    fn arena_alloc_object_handles_null_arena() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Null arena must return MoltObject::none().bits() rather than panic.
        let bits = molt_arena_alloc_object(std::ptr::null_mut(), 16);
        assert_eq!(bits, MoltObject::none().bits());
    }

    #[test]
    fn arena_alloc_object_alignment_and_isolation() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let arena = molt_arena_new();
            let bits1 = molt_arena_alloc_object(arena, 32);
            let bits2 = molt_arena_alloc_object(arena, 32);
            let p1 = MoltObject::from_bits(bits1).as_ptr().unwrap() as usize;
            let p2 = MoltObject::from_bits(bits2).as_ptr().unwrap() as usize;
            assert_ne!(p1, p2, "consecutive arena allocs must not alias");
            assert_eq!(p1 % 8, 0, "arena obj ptr must be 8-byte aligned");
            assert_eq!(p2 % 8, 0, "arena obj ptr must be 8-byte aligned");
            // Header is 24 bytes, payload 32 bytes — consecutive object
            // pointers must be at least header + payload apart so the
            // memory regions cannot overlap.
            let distance = p1.abs_diff(p2);
            assert!(
                distance >= size_of::<MoltHeader>() + 32,
                "arena allocations must not overlap: distance={distance}"
            );
            molt_arena_free(arena);
        });
    }
}
