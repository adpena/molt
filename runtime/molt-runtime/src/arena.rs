use std::mem::{align_of, size_of};

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

/// Per-scope bump allocator for NoEscape values.
///
/// All allocations are 8-byte aligned. At scope exit the entire arena is
/// freed in O(1) by resetting the bump pointer (or dropping the arena).
/// Chunks are allocated on demand and reused across resets.
pub struct ScopeArena {
    /// Backing storage. Each entry is a heap-allocated chunk.
    chunks: Vec<Vec<u8>>,
    /// Next free byte in the current (last) chunk.
    current: *mut u8,
    /// Bytes remaining in the current chunk.
    remaining: usize,
}

impl ScopeArena {
    pub fn new() -> Self {
        let mut chunk = Vec::<u8>::with_capacity(SCOPE_ARENA_CHUNK_SIZE);
        let ptr = chunk.as_mut_ptr();
        let cap = chunk.capacity();
        Self {
            chunks: vec![chunk],
            current: ptr,
            remaining: cap,
        }
    }

    /// Bump-allocate `size` bytes with 8-byte alignment.
    ///
    /// Returns a null pointer only if `size` is zero.
    #[inline]
    pub fn alloc(&mut self, size: usize) -> *mut u8 {
        if size == 0 {
            return std::ptr::null_mut();
        }
        let aligned_size = (size + 7) & !7;
        if aligned_size <= self.remaining {
            let ptr = self.current;
            // SAFETY: `current` points into a live Vec allocation and
            // `aligned_size <= remaining` guarantees we stay in bounds.
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
        let mut chunk = Vec::<u8>::with_capacity(chunk_cap);
        let ptr = chunk.as_mut_ptr();
        let cap = chunk.capacity();
        // SAFETY: `aligned_size <= cap` by construction above.
        self.current = unsafe { ptr.add(aligned_size) };
        self.remaining = cap - aligned_size;
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
            self.remaining = first.capacity();
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
