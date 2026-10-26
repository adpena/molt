//! Canonical inline layouts for tee readers and their GC-owned cache blocks.
//!
//! Slots are NaN-boxed owned edges, never native pointers. Both widths use the
//! same C layout; allocation, traversal and retirement import these types.

/// CPython 3.12--3.15 uses 57 values per link. Retaining this granularity gives
/// the same retirement boundary while amortizing allocation over 57 yields.
pub const TEE_BLOCK_CAPACITY: usize = 57;

#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct TeeCursor {
    pub block_bits: u64,
    pub index: u32,
}

#[repr(C, align(8))]
pub struct TeeBlock {
    pub iterator_bits: u64,
    pub next_bits: u64,
    pub values: [u64; TEE_BLOCK_CAPACITY],
    pub len: u32,
    pub running: u32,
}

impl TeeBlock {
    /// Takes one owned iterator reference; zero slots own no reference.
    pub fn new(iterator_bits: u64) -> Self {
        Self {
            iterator_bits,
            next_bits: 0,
            values: [0; TEE_BLOCK_CAPACITY],
            len: 0,
            running: 0,
        }
    }
}

const _: () = {
    assert!(std::mem::offset_of!(TeeCursor, block_bits) == 0);
    assert!(std::mem::offset_of!(TeeCursor, index) == 8);
    assert!(std::mem::size_of::<TeeCursor>() == 16);
    assert!(std::mem::size_of::<TeeBlock>() == 480);
    assert!(std::mem::align_of::<TeeCursor>() <= 8);
    assert!(std::mem::align_of::<TeeBlock>() <= 8);
};
