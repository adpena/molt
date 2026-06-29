use std::sync::{Mutex, OnceLock};

/// Rarely-accessed per-object metadata, stored in a slab keyed by
/// `MoltHeader::cold_idx`.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct MoltColdHeader {
    /// Function pointer for polling (generators / async tasks).
    pub(crate) poll_fn: u64,
    /// State machine state (generators / async tasks / hash cache).
    pub(crate) state: i64,
    /// Exact allocation size for objects that exceed the size-class table.
    pub(crate) extended_size: usize,
}

/// Slab allocator for cold headers. Entries are stored in a contiguous `Vec`
/// and referenced by a `u32` index stored in `MoltHeader::cold_idx`.
/// Index 0 is reserved as "no cold header". This gives O(1) alloc, access, and
/// free: no hashing, no hash collisions, better cache locality.
struct ColdHeaderSlab {
    /// Slot 0 is unused (sentinel). Valid indices start at 1.
    entries: Vec<MoltColdHeader>,
    /// Slot liveness mirrors `entries` exactly. A freed slot may be reused only
    /// after `free` has marked it non-live; this makes double-free detection a
    /// slab invariant instead of an allocator-side accident.
    live: Vec<bool>,
    /// Free-list of previously freed indices (LIFO reuse).
    free_list: Vec<u32>,
}

impl ColdHeaderSlab {
    fn new() -> Self {
        Self {
            // Slot 0 is the sentinel: push a dummy entry.
            entries: vec![MoltColdHeader::default()],
            live: vec![false],
            free_list: Vec::new(),
        }
    }

    /// Allocate a slot, returning its u32 index (always >= 1).
    fn alloc(&mut self, cold: MoltColdHeader) -> u32 {
        while let Some(idx) = self.free_list.pop() {
            // Belt-and-suspenders: verify the recycled index is in bounds.
            // This defends against any residual free-list corruption.
            if (idx as usize) < self.entries.len() {
                if self.live[idx as usize] {
                    panic!(
                        "cold header slab free-list corruption: live slot {} was queued",
                        idx
                    );
                }
                self.entries[idx as usize] = cold;
                self.live[idx as usize] = true;
                return idx;
            }
            // Index was stale/corrupted: discard and fall through to push.
        }
        let idx = self.entries.len();
        if idx > u32::MAX as usize {
            // Slab full: too many live cold-header users. Panic instead of
            // returning 0, which would silently corrupt object state
            // (`cold_idx = 0` is the "no header" sentinel).
            panic!(
                "cold header slab exhausted ({} entries) - too many live \
                 cold-header users",
                self.entries.len()
            );
        }
        self.entries.push(cold);
        self.live.push(true);
        idx as u32
    }

    /// Get a reference to the cold header at `idx`.
    /// Returns `None` for index 0 (no cold header).
    #[inline]
    fn get(&self, idx: u32) -> Option<&MoltColdHeader> {
        if idx == 0 {
            None
        } else if self.live.get(idx as usize).copied().unwrap_or(false) {
            self.entries.get(idx as usize)
        } else {
            None
        }
    }

    /// Get a mutable reference to the cold header at `idx`.
    /// Returns `None` for index 0 (no cold header).
    #[inline]
    fn get_mut(&mut self, idx: u32) -> Option<&mut MoltColdHeader> {
        if idx == 0 {
            None
        } else if self.live.get(idx as usize).copied().unwrap_or(false) {
            self.entries.get_mut(idx as usize)
        } else {
            None
        }
    }

    /// Free the slot at `idx`, returning it to the free list.
    /// No-op for index 0.
    fn free(&mut self, idx: u32) {
        if idx == 0 {
            return;
        }
        // Zero out the entry to avoid stale data, then recycle. Only push to
        // free_list when the index is actually in bounds: a corrupted cold_idx
        // must not poison the free list.
        if (idx as usize) >= self.entries.len() {
            return;
        }
        if !self.live[idx as usize] {
            panic!("cold header slab double free for slot {}", idx);
        }
        if let Some(entry) = self.entries.get_mut(idx as usize) {
            *entry = MoltColdHeader::default();
            self.live[idx as usize] = false;
            self.free_list.push(idx);
        }
    }
}

static COLD_HEADER_SLAB: OnceLock<Mutex<ColdHeaderSlab>> = OnceLock::new();

fn cold_header_slab() -> &'static Mutex<ColdHeaderSlab> {
    COLD_HEADER_SLAB.get_or_init(|| Mutex::new(ColdHeaderSlab::new()))
}

/// Allocate a cold header, returning its slab index.
/// The caller must store this index in `MoltHeader::cold_idx`.
pub(crate) fn alloc_cold_header(cold: MoltColdHeader) -> u32 {
    let mut slab = cold_header_slab().lock().unwrap();
    slab.alloc(cold)
}

/// High bit of `MoltHeader::cold_idx` flagged as "shared": many instances of
/// the same class point to one cold header allocated at first instantiation.
///
/// Shared cold headers eliminate per-instance cold-header mutex contention in
/// tight allocation loops. For stack-allocated instances, they also encode
/// `class_bits` in the cold header's `state` field without per-instance heap
/// allocation.
///
/// When the bit is set:
/// - `object_state` masks it off before slab lookup.
/// - `object_set_state` allocates a private cold header for the mutating
///   instance, since shared state cannot be modified without affecting siblings.
/// - `object_set_poll_fn` similarly promotes shared to private.
/// - Instance deallocation does not free the shared cold header. The class owns
///   and frees it.
pub(crate) const SHARED_COLD_IDX_BIT: u32 = 1 << 31;

/// Mask off the shared-bit to recover the real slab index.
#[inline]
pub(crate) fn cold_idx_real(raw: u32) -> u32 {
    raw & !SHARED_COLD_IDX_BIT
}

/// Returns `true` when the cold_idx is flagged as shared and should not be
/// freed when the owning instance is deallocated.
#[inline]
pub(crate) fn cold_idx_is_shared(raw: u32) -> bool {
    raw & SHARED_COLD_IDX_BIT != 0
}

/// Retrieve a copy of the cold header at `idx`.
/// Returns `None` if idx == 0.
#[inline]
pub(crate) fn get_cold_header(idx: u32) -> Option<MoltColdHeader> {
    if idx == 0 {
        return None;
    }
    let slab = cold_header_slab().lock().unwrap();
    slab.get(idx).copied()
}

/// Mutate a live cold header at `idx`.
/// Returns `None` if idx == 0, stale, or already freed.
#[inline]
pub(crate) fn with_cold_header_mut<R>(
    idx: u32,
    f: impl FnOnce(&mut MoltColdHeader) -> R,
) -> Option<R> {
    if idx == 0 {
        return None;
    }
    let mut slab = cold_header_slab().lock().unwrap();
    slab.get_mut(idx).map(f)
}

/// Free the cold header at `idx`, returning the slot to the free list.
/// No-op if idx == 0.
pub(crate) fn free_cold_header(idx: u32) {
    if idx == 0 {
        return;
    }
    let mut slab = cold_header_slab().lock().unwrap();
    slab.free(idx);
}

#[cfg(test)]
mod tests {
    use super::{ColdHeaderSlab, MoltColdHeader};

    #[test]
    fn cold_header_slab_rejects_out_of_bounds_free() {
        let mut slab = ColdHeaderSlab::new();
        let idx1 = slab.alloc(MoltColdHeader::default());
        assert!(idx1 >= 1);
        let idx2 = slab.alloc(MoltColdHeader::default());
        assert!(idx2 >= 1);
        let len_before_free = slab.entries.len();
        let free_list_len_before = slab.free_list.len();

        slab.free(24427);

        assert_eq!(slab.free_list.len(), free_list_len_before);
        assert_eq!(slab.entries.len(), len_before_free);
        assert_eq!(slab.live.len(), len_before_free);

        let idx3 = slab.alloc(MoltColdHeader::default());
        assert!(idx3 >= 1);

        slab.free(idx1);
        assert_eq!(slab.free_list.len(), free_list_len_before + 1);
        assert!(slab.get(idx1).is_none());
        let idx4 = slab.alloc(MoltColdHeader::default());
        assert_eq!(idx4, idx1);
        assert!(slab.get(idx4).is_some());
    }

    #[test]
    #[should_panic(expected = "cold header slab double free")]
    fn cold_header_slab_rejects_double_free() {
        let mut slab = ColdHeaderSlab::new();
        let idx = slab.alloc(MoltColdHeader::default());

        slab.free(idx);
        slab.free(idx);
    }

    #[test]
    fn cold_header_slab_supports_more_than_65535_live_entries() {
        let result = std::panic::catch_unwind(|| {
            let mut slab = ColdHeaderSlab::new();
            for _ in 0..70_000 {
                let _ = slab.alloc(MoltColdHeader::default());
            }
            slab.entries.len()
        });

        match result {
            Ok(len) => assert!(
                len > 65_536,
                "expected slab to hold more than 65,536 entries, got {len}"
            ),
            Err(_) => panic!("cold header slab should scale beyond 65,535 live entries"),
        }
    }
}
