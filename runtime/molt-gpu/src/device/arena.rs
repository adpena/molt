//! Arena allocator for temporary GPU buffer allocation.
//!
//! Pre-allocates a memory pool of configurable size and hands out
//! sub-allocations via a bump pointer. The arena is reset after each
//! realize() call, eliminating per-kernel malloc/free overhead.
//!
//! Thread safety: the arena is internally synchronized via a Mutex.
//! This is acceptable because arena operations are O(1) bump-pointer
//! advances, not contended hot paths.
//!
//! Alignment: the arena supports element-size-aware alignment. When
//! allocating for a specific element type (e.g., f32 = 4 bytes),
//! the allocation is aligned to at least that element size, clamped
//! to the arena's minimum alignment (default: 256 bytes for cache-line
//! and GPU buffer alignment).

use std::sync::Mutex;

/// Configuration for the arena allocator.
#[derive(Debug, Clone, Copy)]
pub struct ArenaConfig {
    /// Total pool size in bytes. Default: 64 MiB.
    pub pool_size: usize,
    /// Minimum alignment for sub-allocations in bytes. Must be a power of 2.
    /// Default: 256 (cache-line aligned, suitable for GPU buffers).
    /// Actual alignment may be larger when `alloc_aligned()` requests it.
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
    /// High water mark: maximum cursor value across all generations.
    high_water_mark: usize,
    /// Peak allocation count across all generations.
    peak_alloc_count: usize,
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
                high_water_mark: 0,
                peak_alloc_count: 0,
            }),
            config,
        }
    }

    /// Create a new arena with default configuration (64 MiB, 256-byte alignment).
    pub fn with_defaults() -> Self {
        Self::new(ArenaConfig::default())
    }

    /// Allocate `size` bytes from the arena with the default minimum alignment.
    ///
    /// Returns `Some(ArenaAlloc)` on success, `None` if the arena is full.
    /// The returned allocation is aligned to `config.alignment`.
    #[inline(always)]
    pub fn alloc(&self, size: usize) -> Option<ArenaAlloc> {
        self.alloc_aligned(size, self.config.alignment)
    }

    /// Allocate `size` bytes with a specific alignment requirement.
    ///
    /// The actual alignment used is `max(requested_align, config.alignment)`,
    /// ensuring that all allocations meet the arena's minimum alignment
    /// (for GPU buffer compatibility) while also satisfying element-specific
    /// alignment needs.
    ///
    /// Returns `Some(ArenaAlloc)` on success, `None` if the arena is full.
    #[inline(always)]
    pub fn alloc_aligned(&self, size: usize, requested_align: usize) -> Option<ArenaAlloc> {
        if size == 0 {
            return Some(ArenaAlloc { offset: 0, size: 0 });
        }

        debug_assert!(
            requested_align.is_power_of_two(),
            "requested alignment must be a power of 2"
        );

        let mut state = self.state.lock().unwrap();
        // Use the larger of the requested alignment and the arena's minimum.
        let alignment = requested_align.max(self.config.alignment);

        // Align cursor up to the required alignment.
        let aligned_cursor = (state.cursor + alignment - 1) & !(alignment - 1);
        let end = aligned_cursor + size;

        if end > state.pool.len() {
            return None; // Arena full
        }

        state.cursor = end;
        state.alloc_count += 1;
        state.bytes_allocated += size;

        // Update high water mark
        if state.cursor > state.high_water_mark {
            state.high_water_mark = state.cursor;
        }
        if state.alloc_count > state.peak_alloc_count {
            state.peak_alloc_count = state.alloc_count;
        }

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

    /// Returns the high water mark: the maximum cursor value observed
    /// across all generations. This represents peak memory usage.
    pub fn high_water_mark(&self) -> usize {
        self.state.lock().unwrap().high_water_mark
    }

    /// Returns the peak allocation count observed across all generations.
    pub fn peak_alloc_count(&self) -> usize {
        self.state.lock().unwrap().peak_alloc_count
    }

    /// Returns the current fragmentation ratio as a value in [0.0, 1.0].
    ///
    /// Fragmentation is computed as the ratio of alignment padding bytes
    /// to total cursor bytes. A value of 0.0 means no wasted space; 1.0
    /// means all space is wasted (impossible in practice).
    ///
    /// For a bump allocator, "fragmentation" is strictly the padding
    /// inserted for alignment between allocations.
    pub fn fragmentation(&self) -> f64 {
        let state = self.state.lock().unwrap();
        if state.cursor == 0 {
            return 0.0;
        }
        let padding = state.cursor.saturating_sub(state.bytes_allocated);
        padding as f64 / state.cursor as f64
    }

    /// Returns a snapshot of the pool statistics for diagnostics.
    pub fn stats(&self) -> ArenaStats {
        let state = self.state.lock().unwrap();
        let padding = state.cursor.saturating_sub(state.bytes_allocated);
        ArenaStats {
            pool_size: self.config.pool_size,
            bytes_used: state.cursor,
            bytes_allocated: state.bytes_allocated,
            padding_bytes: padding,
            alloc_count: state.alloc_count,
            generation: state.generation,
            high_water_mark: state.high_water_mark,
            peak_alloc_count: state.peak_alloc_count,
            fragmentation: if state.cursor > 0 {
                padding as f64 / state.cursor as f64
            } else {
                0.0
            },
        }
    }
}

/// Snapshot of arena pool statistics for diagnostics.
#[derive(Debug, Clone)]
pub struct ArenaStats {
    /// Total pool capacity in bytes.
    pub pool_size: usize,
    /// Current cursor position (bytes consumed including padding).
    pub bytes_used: usize,
    /// Total bytes of actual allocation data (excluding padding).
    pub bytes_allocated: usize,
    /// Bytes wasted as alignment padding.
    pub padding_bytes: usize,
    /// Number of active allocations in this generation.
    pub alloc_count: usize,
    /// Current generation (number of resets).
    pub generation: u64,
    /// Peak cursor value across all generations.
    pub high_water_mark: usize,
    /// Peak allocation count across all generations.
    pub peak_alloc_count: usize,
    /// Fragmentation ratio [0.0, 1.0].
    pub fragmentation: f64,
}
