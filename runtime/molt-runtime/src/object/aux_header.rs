use std::sync::atomic::Ordering;

use super::MoltAuxWord;
use crate::{
    AUX_SIDECAR_ALLOC_FAILURE_COUNT, AUX_SIDECAR_BYTES, AUX_SIDECAR_COUNT, AUX_SIDECAR_FREE_BYTES,
    AUX_SIDECAR_FREE_COUNT, profile_hit_bytes_unchecked, profile_hit_unchecked,
};

/// Stable, per-object metadata used only when the hot header's single aux word
/// cannot represent the object's complete auxiliary state.
///
/// A sidecar is allocated while the object is still unpublished. Its address
/// never changes and it is reclaimed exactly once, at object death. The three
/// mutable lanes are atomic so readers do not depend on the GIL for memory
/// safety; `extended_size` is immutable after construction.
#[repr(C)]
pub(crate) struct MoltAuxSidecar {
    pub(crate) class_edge: MoltAuxWord,
    pub(crate) poll_fn: MoltAuxWord,
    pub(crate) state: MoltAuxWord,
    pub(crate) extended_size: usize,
}

impl MoltAuxSidecar {
    #[inline]
    pub(crate) fn new(class_edge: u64, poll_fn: u64, state: i64, extended_size: usize) -> Self {
        Self {
            class_edge: MoltAuxWord::new(class_edge),
            poll_fn: MoltAuxWord::new(poll_fn),
            state: MoltAuxWord::new(state as u64),
            extended_size,
        }
    }

    #[inline]
    pub(crate) fn class_edge(&self) -> u64 {
        self.class_edge.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn poll_fn(&self) -> u64 {
        self.poll_fn.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn state(&self) -> i64 {
        self.state.load(Ordering::Acquire) as i64
    }
}

/// Allocate a sidecar and return its stable address as the header aux word.
#[inline]
pub(crate) fn alloc_aux_sidecar(sidecar: MoltAuxSidecar) -> Option<u64> {
    let layout = std::alloc::Layout::new::<MoltAuxSidecar>();
    if crate::resource::with_tracker(|tracker| tracker.on_allocate(layout.size())).is_err() {
        profile_hit_unchecked(&AUX_SIDECAR_ALLOC_FAILURE_COUNT);
        return None;
    }
    let ptr = unsafe { std::alloc::alloc(layout) as *mut MoltAuxSidecar };
    if ptr.is_null() {
        let _ = crate::resource::try_with_tracker(|tracker| tracker.on_free(layout.size()));
        profile_hit_unchecked(&AUX_SIDECAR_ALLOC_FAILURE_COUNT);
        return None;
    }
    unsafe {
        ptr.write(sidecar);
    }
    profile_hit_unchecked(&AUX_SIDECAR_COUNT);
    profile_hit_bytes_unchecked(&AUX_SIDECAR_BYTES, layout.size() as u64);
    Some(ptr.expose_provenance() as u64)
}

/// Resolve a sidecar address previously returned by `alloc_aux_sidecar`.
///
/// # Safety
/// `word` must be the live aux word of a header whose kind is SIDECAR.
#[inline]
pub(crate) unsafe fn aux_sidecar_from_word(word: u64) -> &'static MoltAuxSidecar {
    debug_assert_ne!(word, 0, "SIDECAR aux word must carry a live address");
    unsafe { &*std::ptr::with_exposed_provenance::<MoltAuxSidecar>(word as usize) }
}

/// Reclaim a sidecar at object death.
///
/// # Safety
/// The owning object must be terminally unreachable, and `word` must not have
/// been freed previously. Object lifetime/RC is the reclamation authority: no
/// independent sidecar references may outlive the object.
#[inline]
pub(crate) unsafe fn free_aux_sidecar(word: u64) {
    if word != 0 {
        let layout = std::alloc::Layout::new::<MoltAuxSidecar>();
        let ptr = std::ptr::with_exposed_provenance_mut::<MoltAuxSidecar>(word as usize);
        unsafe {
            ptr.drop_in_place();
            std::alloc::dealloc(ptr.cast::<u8>(), layout);
        }
        let _ = crate::resource::try_with_tracker(|tracker| tracker.on_free(layout.size()));
        profile_hit_unchecked(&AUX_SIDECAR_FREE_COUNT);
        profile_hit_bytes_unchecked(&AUX_SIDECAR_FREE_BYTES, layout.size() as u64);
    }
}

#[inline]
pub(crate) const fn aux_sidecar_size() -> usize {
    std::mem::size_of::<MoltAuxSidecar>()
}

const _: () = {
    assert!(std::mem::align_of::<MoltAuxSidecar>() >= std::mem::align_of::<MoltAuxWord>());
};
