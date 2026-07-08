use std::mem::{align_of, size_of};

fn release_tracked_bytes(size: usize) {
    let _ = crate::try_with_tracker(|tracker| tracker.on_free(size));
}

pub struct TempArena {
    chunk_size: usize,
    chunks: Vec<Vec<u8>>,
    offset: usize,
    charged_bytes: usize,
}

impl TempArena {
    pub fn new(chunk_size: usize) -> Self {
        let size = chunk_size.max(1024);
        let mut chunks = Vec::new();
        let mut charged_bytes = 0;
        if let Some(chunk) = Self::try_alloc_chunk(size) {
            if chunks.try_reserve(1).is_ok() {
                charged_bytes = chunk.capacity();
                chunks.push(chunk);
            } else {
                release_tracked_bytes(chunk.capacity());
            }
        }
        Self {
            chunk_size: size,
            chunks,
            offset: 0,
            charged_bytes,
        }
    }

    pub fn reset(&mut self) {
        if self.chunks.is_empty() {
            if let Some(chunk) = Self::try_alloc_chunk(self.chunk_size) {
                if self.chunks.try_reserve(1).is_ok() {
                    self.charged_bytes = self.charged_bytes.saturating_add(chunk.capacity());
                    self.chunks.push(chunk);
                } else {
                    release_tracked_bytes(chunk.capacity());
                }
            }
        } else {
            while self.chunks.len() > 1 {
                if let Some(chunk) = self.chunks.pop() {
                    self.charged_bytes = self.charged_bytes.saturating_sub(chunk.capacity());
                    release_tracked_bytes(chunk.capacity());
                }
            }
        }
        self.offset = 0;
    }

    pub fn clear(&mut self) {
        self.release_all_chunks();
        self.chunks.clear();
        self.offset = 0;
    }

    /// Release ALL heap memory, including the outer Vec's buffer.
    /// After this call, dropping `self` will not invoke the allocator.
    pub fn drain(&mut self) {
        self.release_all_chunks();
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
        if self.chunks.is_empty() {
            let new_size = self.chunk_size.max(size);
            let Some(chunk) = Self::try_alloc_chunk(new_size) else {
                return std::ptr::null_mut();
            };
            if self.chunks.try_reserve(1).is_err() {
                release_tracked_bytes(chunk.capacity());
                return std::ptr::null_mut();
            }
            self.charged_bytes = self.charged_bytes.saturating_add(chunk.capacity());
            self.chunks.push(chunk);
            self.offset = 0;
        }
        let Some(aligned) = self
            .offset
            .checked_add(align - 1)
            .map(|val| val & !(align - 1))
        else {
            return std::ptr::null_mut();
        };
        let Some(needed) = aligned.checked_add(size) else {
            return std::ptr::null_mut();
        };
        if needed > self.chunks.last().map(|chunk| chunk.len()).unwrap_or(0) {
            let new_size = self.chunk_size.max(size);
            let Some(chunk) = Self::try_alloc_chunk(new_size) else {
                return std::ptr::null_mut();
            };
            if self.chunks.try_reserve(1).is_err() {
                release_tracked_bytes(chunk.capacity());
                return std::ptr::null_mut();
            }
            self.charged_bytes = self.charged_bytes.saturating_add(chunk.capacity());
            self.chunks.push(chunk);
            self.offset = 0;
            return self.alloc_slice::<T>(len);
        }
        let ptr = unsafe { self.chunks.last_mut().unwrap().as_mut_ptr().add(aligned) };
        self.offset = needed;
        ptr as *mut T
    }

    fn try_alloc_chunk(size: usize) -> Option<Vec<u8>> {
        if crate::with_tracker(|tracker| tracker.on_allocate(size)).is_err() {
            return None;
        }
        let mut chunk = Vec::new();
        if chunk.try_reserve_exact(size).is_err() {
            release_tracked_bytes(size);
            return None;
        }
        chunk.resize(size, 0);
        let capacity = chunk.capacity();
        if capacity > size
            && crate::with_tracker(|tracker| tracker.on_grow(capacity - size)).is_err()
        {
            release_tracked_bytes(size);
            return None;
        }
        Some(chunk)
    }

    fn release_all_chunks(&mut self) {
        for chunk in &self.chunks {
            release_tracked_bytes(chunk.capacity());
        }
        self.charged_bytes = 0;
    }
}

impl Drop for TempArena {
    fn drop(&mut self) {
        self.release_all_chunks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    struct TrackerReset;

    impl Drop for TrackerReset {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    #[test]
    fn temp_arena_respects_initial_resource_limit_without_aborting() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(1023),
            ..Default::default()
        })));
        let _reset = TrackerReset;

        let mut arena = TempArena::new(1024);
        assert!(arena.chunks.is_empty());
        let ptr = arena.alloc_slice::<u8>(1);
        assert!(ptr.is_null());
    }

    #[test]
    fn denied_temp_arena_growth_does_not_poison_existing_chunk() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(1024),
            ..Default::default()
        })));
        let _reset = TrackerReset;

        let mut arena = TempArena::new(1024);
        assert_eq!(arena.chunks.len(), 1);
        let denied = arena.alloc_slice::<u8>(2048);
        assert!(denied.is_null());

        let allowed = arena.alloc_slice::<u8>(8);
        assert!(
            !allowed.is_null(),
            "denied TempArena growth must leave the current chunk usable"
        );
    }
}
