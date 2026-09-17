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
/// never changes and it is reclaimed exactly once, at object death. The
/// mutable lanes are atomic so readers do not depend on the GIL for memory
/// safety; `extended_size` is immutable after construction.
#[repr(C)]
pub(crate) struct MoltAuxSidecar {
    pub(crate) class_edge: MoltAuxWord,
    pub(crate) poll_fn: MoltAuxWord,
    pub(crate) state: MoltAuxWord,
    pub(crate) shape: MoltAuxWord,
    /// Owned Python globals captured when a compiled suspended frame is created.
    /// Zero denotes a runtime task without a Python namespace.
    frame_globals: MoltAuxWord,
    /// Builtins selected by the creation activation, not by a later globals lookup.
    frame_builtins: MoltAuxWord,
    /// Exact code object captured at construction, independent of symbol rebinding.
    frame_code: MoltAuxWord,
    pub(crate) extended_size: usize,
}

impl MoltAuxSidecar {
    #[inline]
    pub(crate) fn new(class_edge: u64, poll_fn: u64, state: i64, extended_size: usize) -> Self {
        Self {
            class_edge: MoltAuxWord::new(class_edge),
            poll_fn: MoltAuxWord::new(poll_fn),
            state: MoltAuxWord::new(state as u64),
            shape: MoltAuxWord::new(0),
            frame_globals: MoltAuxWord::new(0),
            frame_builtins: MoltAuxWord::new(0),
            frame_code: MoltAuxWord::new(0),
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

    #[inline]
    pub(crate) fn shape(&self) -> u16 {
        self.shape.load(Ordering::Acquire) as u16
    }
}

/// Borrow the globals, builtins and exact code retained by the suspended activation.
#[inline]
pub(crate) fn object_frame_context_bits(ptr: *mut u8) -> [u64; 3] {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return [0; 3];
    }
    let sidecar = unsafe { super::sidecar_from_snapshot(snapshot) };
    [
        sidecar.frame_globals.load(Ordering::Acquire),
        sidecar.frame_builtins.load(Ordering::Acquire),
        sidecar.frame_code.load(Ordering::Acquire),
    ]
}

#[inline]
pub(crate) fn object_frame_code_bits(ptr: *mut u8) -> u64 {
    object_frame_context_bits(ptr)[2]
}

#[inline]
#[cfg(test)]
pub(crate) fn object_frame_globals_bits(ptr: *mut u8) -> u64 {
    object_frame_context_bits(ptr)[0]
}

#[inline]
#[cfg(test)]
pub(crate) fn object_frame_builtins_bits(ptr: *mut u8) -> u64 {
    object_frame_context_bits(ptr)[1]
}

/// Capture borrowed activation context before task publication. An unavailable
/// namespace stays unavailable rather than being recomputed on resume.
pub(crate) unsafe fn object_init_frame_context_unpublished(
    py: &crate::PyToken<'_>,
    ptr: *mut u8,
    globals_bits: u64,
    builtins_bits: u64,
    code_bits: u64,
) -> bool {
    if [globals_bits, builtins_bits, code_bits] == [0; 3] {
        return true;
    }
    unsafe {
        let globals_valid = globals_bits == 0
            || crate::builtins::frames::globals_namespace_storage_bits(py, globals_bits).is_some();
        if !globals_valid && crate::exception_pending(py) {
            return false;
        }
        if !globals_valid
            || (code_bits != 0
                && !crate::obj_from_bits(code_bits)
                    .as_ptr()
                    .is_some_and(|code_ptr| super::object_type_id(code_ptr) == crate::TYPE_ID_CODE))
        {
            crate::raise_exception::<u64>(py, "SystemError", "invalid suspended frame context");
            return false;
        }
        if !super::object_init_sidecar_unpublished(ptr) {
            return false;
        }
        let sidecar = super::sidecar_from_snapshot(super::object_aux_snapshot(ptr));
        debug_assert_eq!(sidecar.frame_globals.load(Ordering::Acquire), 0);
        debug_assert_eq!(sidecar.frame_builtins.load(Ordering::Acquire), 0);
        debug_assert_eq!(sidecar.frame_code.load(Ordering::Acquire), 0);
        for bits in [globals_bits, builtins_bits, code_bits] {
            if bits != 0 {
                crate::inc_ref_bits(py, bits);
            }
        }
        sidecar.frame_globals.store(globals_bits, Ordering::Release);
        sidecar
            .frame_builtins
            .store(builtins_bits, Ordering::Release);
        sidecar.frame_code.store(code_bits, Ordering::Release);
        crate::object_mark_has_ptrs(py, ptr);
    }
    true
}

/// Detach every activation owner before any callback-capable releases. Cyclic GC
/// and terminal destruction use this transfer, so each owner retires once even
/// when globals and builtins are the same dictionary.
pub(crate) unsafe fn object_take_frame_context_bits(ptr: *mut u8) -> [u64; 3] {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return [0; 3];
    }
    let sidecar = unsafe { super::sidecar_from_snapshot(snapshot) };
    [
        sidecar.frame_globals.swap(0, Ordering::AcqRel),
        sidecar.frame_builtins.swap(0, Ordering::AcqRel),
        sidecar.frame_code.swap(0, Ordering::AcqRel),
    ]
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
    let address = usize::try_from(word)
        .expect("runtime-owned sidecar address must fit the active address space");
    unsafe { &*std::ptr::with_exposed_provenance::<MoltAuxSidecar>(address) }
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
        let address = usize::try_from(word)
            .expect("runtime-owned sidecar address must fit the active address space");
        let ptr = std::ptr::with_exposed_provenance_mut::<MoltAuxSidecar>(address);
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

#[cfg(test)]
mod tests {
    #[test]
    fn suspended_namespace_aliases_are_two_owners_with_idempotent_retirement() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            use crate::{MoltObject, alloc_dict_with_pairs, dec_ref_bits};
            let dictionary = alloc_dict_with_pairs(py, &[]);
            assert!(!dictionary.is_null());
            let bits = MoltObject::from_ptr(dictionary).bits();
            let refcount =
                || unsafe { (*crate::header_from_obj_ptr(dictionary)).ref_count_snapshot() };
            let baseline = refcount();
            let name = MoltObject::from_ptr(crate::alloc_string(py, b"suspended-context")).bits();
            let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            let code = crate::object::builders::alloc_code_obj(
                py,
                name,
                name,
                1,
                MoltObject::none().bits(),
                empty,
                empty,
                0,
                0,
                0,
            );
            let code_bits = MoltObject::from_ptr(code).bits();
            let code_refcount =
                || unsafe { (*crate::header_from_obj_ptr(code)).ref_count_snapshot() };
            let code_baseline = code_refcount();
            dec_ref_bits(py, name);
            dec_ref_bits(py, empty);
            for kind in [
                crate::TASK_KIND_GENERATOR,
                crate::TASK_KIND_COROUTINE,
                crate::TASK_KIND_FUTURE,
            ] {
                for clear_first in [false, true] {
                    let task = crate::molt_task_new(1, crate::GEN_CONTROL_SIZE as u64, kind);
                    let ptr = crate::obj_from_bits(task).as_ptr().unwrap();
                    assert_eq!(super::object_frame_context_bits(ptr), [0; 3]);
                    assert!(unsafe {
                        super::object_init_frame_context_unpublished(py, ptr, bits, bits, code_bits)
                    });
                    assert_eq!(refcount(), baseline + 2);
                    assert_eq!(code_refcount(), code_baseline + 1);
                    let mut aliases = 0;
                    unsafe {
                        crate::object::heap_lifecycle::visit_owned_values(py, ptr, &mut |edge| {
                            aliases += usize::from(edge == bits);
                        });
                    }
                    assert_eq!(aliases, 2, "GC must count both owned alias edges");
                    let (capacity, _) =
                        unsafe { crate::object::heap_lifecycle::terminal_detach_capacity(py, ptr) };
                    assert!(
                        capacity >= 3,
                        "terminal sink must reserve all context owners"
                    );
                    if clear_first {
                        for _ in 0..2 {
                            unsafe {
                                crate::object::heap_lifecycle::clear_cycle_edges(py, ptr);
                            }
                            assert_eq!(super::object_frame_context_bits(ptr), [0; 3]);
                            assert_eq!(refcount(), baseline);
                            assert_eq!(code_refcount(), code_baseline);
                        }
                    }
                    dec_ref_bits(py, task);
                    assert_eq!(
                        refcount(),
                        baseline,
                        "terminal release must not duplicate GC clear"
                    );
                    assert_eq!(code_refcount(), code_baseline);
                }
            }
            dec_ref_bits(py, bits);
            dec_ref_bits(py, code_bits);
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    #[should_panic(expected = "runtime-owned sidecar address must fit")]
    fn corrupted_sidecar_word_cannot_alias_a_low_address() {
        let malformed = u64::from(u32::MAX) + 1;
        let _ = unsafe { super::aux_sidecar_from_word(malformed) };
    }
}
