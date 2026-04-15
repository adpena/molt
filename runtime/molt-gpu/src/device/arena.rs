//! Arena allocator for temporary GPU buffer allocation.
//!
//! Pre-allocates a memory pool of configurable size and hands out
//! sub-allocations via a bump pointer. The arena is reset after each
//! realize() call, eliminating per-kernel malloc/free overhead.
//!
//! Thread safety: the arena is internally synchronized via a Mutex.
//! This is acceptable because arena operations are O(1) bump-pointer
//! advances, not contended hot paths.

use std::sync::Mutex;

/// Configuration for the arena allocator.
#[derive(Debug, Clone, Copy)]
pub struct ArenaConfig {
    /// Total pool size in bytes. Default: 64 MiB.
    pub pool_size: usize,
    /// Alignment for sub-allocations in bytes. Must be a power of 2.
    /// Default: 256 (cache-line aligned, suitable for GPU buffers).
    pub alignment: usize,
}

impl Default for ArenaConfig {
    fn default() -> Self {
        Self {
            pool_size: 64 * 1024 * 1024, // 64 MiB
            alignment: 256,
        }
    }
}

/// A sub-allocation from the arena: offset and size within the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaAlloc {
    /// Byte offset into the arena's pool.
    pub offset: usize,
    /// Size in bytes of this allocation.
    pub size: usize,
}

/// Internal state of the arena, protected by a Mutex.
struct ArenaState {
    /// The backing memory pool.
    pool: Vec<u8>,
    /// Current bump pointer (next free byte offset).
    cursor: usize,
    /// Number of active allocations (for debugging/metrics).
    alloc_count: usize,
    /// Total bytes allocated in this generation (for metrics).
    bytes_allocated: usize,
    /// Generation counter: incremented on each reset.
    generation: u64,
}

/// Bump-pointer arena allocator for temporary buffers.
///
/// Usage pattern:
/// 1. Create arena with desired pool size.
/// 2. Allocate temporary buffers during kernel execution via `alloc()`.
/// 3. After realize() completes, call `reset()` to reclaim all memory.
///
/// The arena does NOT support individual `free()` calls. All memory
/// is reclaimed at once via `reset()`. This is the correct model for
/// GPU kernel execution where all intermediates are temporary and
/// freed together after the computation graph is realized.
pub struct Arena {
    state: Mutex<ArenaState>,
    config: ArenaConfig,
}

impl Arena {
    /// Create a new arena with the given configuration.
    pub fn new(config: ArenaConfig) -> Self {
        assert!(config.alignment.is_power_of_two(), "alignment must be power of 2");
        assert!(config.pool_size > 0, "pool_size must be > 0");
        Self {
            state: Mutex::new(ArenaState {
                pool: vec![0u8; config.pool_size],
                cursor: 0,
                alloc_count: 0,
                bytes_allocated: 0,
                generation: 0,
            }),
            config,
        }
    }

    /// Create a new arena with default configuration (64 MiB, 256-byte alignment).
    pub fn with_defaults() -> Self {
        Self::new(ArenaConfig::default())
    }

    /// Allocate `size` bytes from the arena.
    ///
    /// Returns `Some(ArenaAlloc)` on success, `None` if the arena is full.
    /// The returned allocation is aligned to `config.alignment`.
    pub fn alloc(&self, size: usize) -> Option<ArenaAlloc> {
        if size == 0 {
            return Some(ArenaAlloc { offset: 0, size: 0 });
        }

        let mut state = self.state.lock().unwrap();
        let alignment = self.config.alignment;

        // Align cursor up to the required alignment.
        let aligned_cursor = (state.cursor + alignment - 1) & !(alignment - 1);
        let end = aligned_cursor + size;

        if end > state.pool.len() {
            return None; // Arena full
        }

        state.cursor = end;
        state.alloc_count += 1;
        state.bytes_allocated += size;

        Some(ArenaAlloc {
            offset: aligned_cursor,
            size,
        })
    }

    /// Get a slice of the arena's pool for the given allocation.
    ///
    /// # Panics
    /// Panics if the allocation is out of bounds.
    pub fn slice(&self, alloc: &ArenaAlloc) -> Vec<u8> {
        let state = self.state.lock().unwrap();
        state.pool[alloc.offset..alloc.offset + alloc.size].to_vec()
    }

    /// Write data into the arena at the given allocation.
    ///
    /// # Panics
    /// Panics if data length exceeds allocation size or allocation is out of bounds.
    pub fn write(&self, alloc: &ArenaAlloc, data: &[u8]) {
        assert!(data.len() <= alloc.size, "data ({}) exceeds allocation ({})", data.len(), alloc.size);
        let mut state = self.state.lock().unwrap();
        state.pool[alloc.offset..alloc.offset + data.len()].copy_from_slice(data);
    }

    /// Reset the arena, reclaiming all allocations.
    ///
    /// This is an O(1) operation (just resets the bump pointer).
    /// All previously returned `ArenaAlloc` handles become invalid.
    pub fn reset(&self) {
        let mut state = self.state.lock().unwrap();
        state.cursor = 0;
        state.alloc_count = 0;
        state.bytes_allocated = 0;
        state.generation += 1;
    }

    /// Returns the number of bytes currently allocated.
    pub fn bytes_used(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.cursor
    }

    /// Returns the total pool capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.config.pool_size
    }

    /// Returns the number of bytes remaining.
    pub fn bytes_remaining(&self) -> usize {
        let state = self.state.lock().unwrap();
        self.config.pool_size.saturating_sub(state.cursor)
    }

    /// Returns the current generation (number of resets).
    pub fn generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }

    /// Returns the number of active allocations.
    pub fn alloc_count(&self) -> usize {
        self.state.lock().unwrap().alloc_count
    }
}
