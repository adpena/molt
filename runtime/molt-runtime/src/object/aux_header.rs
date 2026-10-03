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
    /// Owned active await continuation. Wakeup subscriptions may disappear before
    /// resumption; they do not own Python delegation or cr_await introspection.
    frame_awaited: MoltAuxWord,
    /// Initialized local slots plus the monotonic count of published cells.
    /// `FRAME_LOCALS_PRESTART` marks a prefix a frame proxy initialized before
    /// the compiled prologue ran.
    frame_locals_phase: MoltAuxWord,
    /// Owned `FRAME_BINDINGS` payload of the activation's frame object,
    /// attached on first observation. The activation's completion or death
    /// hands its bindings to it (when an observer shares it) or detaches it
    /// before the task storage it reads goes away.
    frame_bindings: MoltAuxWord,
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
            frame_awaited: MoltAuxWord::new(0),
            frame_locals_phase: MoltAuxWord::new(0),
            frame_bindings: MoltAuxWord::new(0),
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

/// Set in the locals phase while a frame proxy's write, not the compiled
/// prologue, initialized the body prefix of a created activation.
const FRAME_LOCALS_PRESTART: u64 = 1 << 62;

pub(crate) fn object_frame_locals_phase(ptr: *mut u8) -> usize {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return 0;
    }
    (unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_locals_phase
        .load(Ordering::Acquire)
        & !FRAME_LOCALS_PRESTART) as usize
}

/// Whether a frame proxy initialized the body prefix before the prologue.
pub(crate) fn object_frame_locals_prestart(ptr: *mut u8) -> bool {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    snapshot.kind == super::HEADER_AUX_KIND_SIDECAR
        && unsafe { super::sidecar_from_snapshot(snapshot) }
            .frame_locals_phase
            .load(Ordering::Acquire)
            & FRAME_LOCALS_PRESTART
            != 0
}

/// Caller holds the GIL and the compiled invocation's activation owner.
/// Publishing a phase ends a proxy's pre-start initialization.
pub(crate) unsafe fn object_set_frame_locals_phase(ptr: *mut u8, phase: usize) {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    assert_eq!(snapshot.kind, super::HEADER_AUX_KIND_SIDECAR);
    unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_locals_phase
        .store(phase as u64, Ordering::Release);
}

/// A frame proxy initialized a created activation's body prefix (phase 1)
/// before its prologue. Caller holds the GIL and has checked phase 0.
pub(crate) unsafe fn object_mark_frame_locals_prestart(ptr: *mut u8) {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    assert_eq!(snapshot.kind, super::HEADER_AUX_KIND_SIDECAR);
    unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_locals_phase
        .store(1 | FRAME_LOCALS_PRESTART, Ordering::Release);
}

/// Borrow the activation frame object's payload; zero until observed.
#[inline]
pub(crate) fn object_frame_bindings_bits(ptr: *mut u8) -> u64 {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return 0;
    }
    unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_bindings
        .load(Ordering::Acquire)
}

/// Install `payload` (the caller's owned reference moves in) unless a
/// payload is already attached. Returns the attached payload, borrowed; zero
/// when the task has no sidecar, in which case the caller keeps `payload`.
pub(crate) unsafe fn object_install_frame_bindings(ptr: *mut u8, payload: u64) -> u64 {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return 0;
    }
    match unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_bindings
        .compare_exchange(0, payload, Ordering::AcqRel, Ordering::Acquire)
    {
        Ok(_) => payload,
        Err(existing) => existing,
    }
}

/// Detach the frame object's payload; the caller owns the returned reference.
pub(crate) unsafe fn object_take_frame_bindings_bits(ptr: *mut u8) -> u64 {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return 0;
    }
    unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_bindings
        .swap(0, Ordering::AcqRel)
}

/// Borrow the active continuation while holding the task's execution authority.
#[inline]
pub(crate) fn object_frame_awaited_bits(ptr: *mut u8) -> u64 {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return 0;
    }
    unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_awaited
        .load(Ordering::Acquire)
}

/// Transfer a new owned continuation to an existing task. Publish before any
/// callback-capable release; a finalizer may inspect or resume another task.
/// The task must already have its constructor-selected sidecar.
pub(crate) unsafe fn object_replace_frame_awaited_owned(
    py: &crate::PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
) {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    assert_eq!(snapshot.kind, super::HEADER_AUX_KIND_SIDECAR);
    let sidecar = unsafe { super::sidecar_from_snapshot(snapshot) };
    let bits = if crate::obj_from_bits(bits).is_none() {
        0
    } else {
        bits
    };
    let previous = sidecar.frame_awaited.swap(bits, Ordering::AcqRel);
    if previous != 0 {
        crate::dec_ref_bits(py, previous);
    }
}

/// Detach without releasing so GC and terminal cleanup can first publish the
/// complete inert activation, then release all of its owned edges together.
pub(crate) unsafe fn object_take_frame_awaited_bits(ptr: *mut u8) -> u64 {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return 0;
    }
    unsafe { super::sidecar_from_snapshot(snapshot) }
        .frame_awaited
        .swap(0, Ordering::AcqRel)
}

/// A closed frame relinquishes its namespaces while cr_code remains observable.
pub(crate) unsafe fn object_take_frame_namespaces_bits(ptr: *mut u8) -> [u64; 2] {
    let snapshot = unsafe { super::object_aux_snapshot(ptr) };
    if snapshot.kind != super::HEADER_AUX_KIND_SIDECAR {
        return [0; 2];
    }
    let sidecar = unsafe { super::sidecar_from_snapshot(snapshot) };
    [
        sidecar.frame_globals.swap(0, Ordering::AcqRel),
        sidecar.frame_builtins.swap(0, Ordering::AcqRel),
    ]
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
    fn active_await_owner_survives_subscription_removal_and_retires_once() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            use crate::{dec_ref_bits, inc_ref_bits};
            let child = crate::molt_task_new(1, 0, crate::TASK_KIND_FUTURE);
            let child_ptr = crate::obj_from_bits(child).as_ptr().unwrap();
            let refcount =
                || unsafe { (*crate::header_from_obj_ptr(child_ptr)).ref_count_snapshot() };
            let baseline = refcount();
            for clear_first in [false, true] {
                let parent = crate::molt_task_new(1, 0, crate::TASK_KIND_COROUTINE);
                let parent_ptr = crate::obj_from_bits(parent).as_ptr().unwrap();
                inc_ref_bits(py, child);
                unsafe { super::object_replace_frame_awaited_owned(py, parent_ptr, child) };
                assert_eq!(refcount(), baseline + 1);
                crate::await_waiter_register(py, parent_ptr, child_ptr);
                crate::await_waiter_clear(py, parent_ptr);
                assert_eq!(super::object_frame_awaited_bits(parent_ptr), child);
                assert_eq!(refcount(), baseline + 1);
                let mut occurrences = 0;
                unsafe {
                    crate::object::heap_lifecycle::visit_owned_values(
                        py,
                        parent_ptr,
                        &mut |edge| {
                            occurrences += usize::from(edge == child);
                        },
                    );
                }
                assert_eq!(occurrences, 1);
                if clear_first {
                    for _ in 0..2 {
                        unsafe { crate::object::heap_lifecycle::clear_cycle_edges(py, parent_ptr) };
                        assert_eq!(super::object_frame_awaited_bits(parent_ptr), 0);
                        assert_eq!(refcount(), baseline);
                    }
                }
                dec_ref_bits(py, parent);
                assert_eq!(refcount(), baseline);
            }
            dec_ref_bits(py, child);
            assert!(!crate::exception_pending(py));
        });
    }

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
