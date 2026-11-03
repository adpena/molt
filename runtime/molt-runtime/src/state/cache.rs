use crate::PyToken;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::RuntimeState;
use crate::{MoltObject, dec_ref_bits};

macro_rules! define_method_cache {
    (@unit $field:ident) => {
        ()
    };
    ($($field:ident),+ $(,)?) => {
        const METHOD_CACHE_SLOT_COUNT: usize = <[()]>::len(&[
            $(define_method_cache!(@unit $field)),+
        ]);

        pub(crate) struct MethodCache {
            $(pub(crate) $field: AtomicU64,)+
        }

        impl MethodCache {
            pub(crate) fn new() -> Self {
                Self {
                    $($field: AtomicU64::new(0),)+
                }
            }

            fn slots(&self) -> Vec<&AtomicU64> {
                let mut slots = Vec::with_capacity(METHOD_CACHE_SLOT_COUNT);
                $(slots.push(&self.$field);)+
                slots
            }
        }
    };
}

define_method_cache! {
    function_descriptor_get,
}

#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial"))]
pub(crate) fn intern_bridge_protocol_name(
    _py: &PyToken<'_>,
    key: &[u8],
) -> Result<Option<u64>, ()> {
    if crate::exception_pending(_py) {
        return Err(());
    }
    let Some(name) = crate::NameId::from_bytes(key) else {
        return Ok(None);
    };
    if !matches!(
        name,
        crate::NameId::DunderFloat
            | crate::NameId::DunderIndex
            | crate::NameId::DunderTrunc
            | crate::NameId::DunderCeil
            | crate::NameId::DunderFloor
            | crate::NameId::DunderRound
            | crate::NameId::DunderInt
            | crate::NameId::DunderBool
            | crate::NameId::DunderAbs
            | crate::NameId::DunderLen
    ) {
        return Ok(None);
    }
    let bits = name.bits(_py);
    if bits == 0 || crate::exception_pending(_py) {
        Err(())
    } else {
        Ok(Some(bits))
    }
}

/// Allocate one owned keyword-marker object for a runtime cache publication.
/// Zero is the cache initialization failure value; Python None is never a
/// marker candidate. Both itertools bridge profiles and functools use this
/// allocator, preserving the original allocation error and leaving retry open.
pub(crate) fn alloc_kwd_mark(py: &PyToken<'_>) -> u64 {
    if crate::exception_pending(py) {
        return 0;
    }
    let ptr = crate::alloc_object(
        py,
        std::mem::size_of::<crate::MoltHeader>(),
        crate::TYPE_ID_OBJECT,
    );
    if ptr.is_null() {
        0
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// Give a Python-callable result its own reference to a borrowed cached handle.
/// Cache initialization transfers an owner to the slot; neither a cache hit nor
/// its first publication transfers that owner to the caller. A zero handle is
/// failed initialization: return None without replacing the pending exception.
/// Retaining tuple/dict publication already owns borrowed inputs and does not
/// use this result boundary.
pub(crate) fn retain_cached_result(py: &PyToken<'_>, bits: u64) -> u64 {
    if bits == 0 {
        return MoltObject::none().bits();
    }
    crate::inc_ref_bits(py, bits);
    bits
}

fn release_atomic_cache_owner(_py: &PyToken<'_>, bits: u64) {
    if bits != 0 {
        dec_ref_bits(_py, bits);
    }
}

/// Return whether a populated slot was detached, including interned values
/// whose physical lifetime remains owned by the shutdown singleton authority.
#[cfg(test)]
pub(crate) fn clear_atomic_bits(_py: &PyToken<'_>, slot: &AtomicU64) -> bool {
    clear_atomic_slots(_py, &[slot])
}

pub(crate) fn clear_atomic_slots(_py: &PyToken<'_>, slots: &[&AtomicU64]) -> bool {
    crate::gil_assert();
    // Publish the entire empty cohort before a displaced owner's finalizer can
    // reenter any sibling cache. New callback publications belong to the next
    // shutdown fixed-point pass, not this detached snapshot.
    let mut detached = Vec::with_capacity(slots.len());
    for slot in slots {
        let bits = slot.swap(0, AtomicOrdering::AcqRel);
        if bits != 0 {
            detached.push(bits);
        }
    }
    let changed = !detached.is_empty();
    for bits in detached {
        release_atomic_cache_owner(_py, bits);
    }
    changed
}

/// Enumerate canonical class anchors only from declared runtime cache owners.
/// Heap immutability alone is never permission to retire an arbitrary class.
pub(crate) fn cached_runtime_class_roots(py: &PyToken<'_>, slots: &[&AtomicU64]) -> Vec<u64> {
    crate::gil_assert();
    slots
        .iter()
        .map(|slot| slot.load(AtomicOrdering::Acquire))
        .filter(|bits| crate::object::class_storage::is_canonical_runtime_class(py, *bits))
        .collect()
}

/// Keep canonical class anchors live through the shared retirement transaction;
/// detach every other cache owner before any callback-capable reference release.
pub(crate) fn clear_cached_runtime_callbacks(py: &PyToken<'_>, slots: &[&AtomicU64]) -> bool {
    crate::gil_assert();
    let callbacks: Vec<_> = slots
        .iter()
        .copied()
        .filter(|slot| {
            !crate::object::class_storage::is_canonical_runtime_class(
                py,
                slot.load(AtomicOrdering::Acquire),
            )
        })
        .collect();
    clear_atomic_slots(py, &callbacks)
}

pub(crate) fn clear_method_cache(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let slots = state.method_cache.slots();
    clear_atomic_slots(_py, &slots)
}

pub(crate) fn clear_runtime_names(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    // Withdraw borrowed reverse-index observations before any object owner.
    *state.names.index.lock().unwrap_or_else(|e| e.into_inner()) = Vec::new();
    let slots = state.names.slots();
    clear_atomic_slots(_py, &slots)
}

#[cfg(test)]
#[path = "cache/tests.rs"]
mod tests;
