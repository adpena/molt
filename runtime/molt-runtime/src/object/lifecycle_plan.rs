//! The class lifecycle plan (call_anatomy.md 4.5, "Implementation (object
//! lifecycle, HF-202)").
//!
//! A class decides once how its instances are released; every free reads the
//! decision with one load instead of walking the MRO or the field layout. The
//! plan is a projection of the class's current facts, never an independent
//! authority:
//!
//! - `class_finish_definition` publishes it when the class seals, and
//! - `class_bump_layout_version`, which every class mutation calls for the
//!   class and each descendant (HF-812), recomputes it there.
//!
//! So between mutations the word equals [`compute`] of the class. A zero word
//! is "no plan": the class is not sealed, and every reader takes the generic
//! path. Debug builds check the word against [`compute`] at each read.

use super::{
    ObjectShapeId, TYPE_ID_OBJECT, TYPE_ID_TYPE, class_definition_is_finished,
    class_header_declares_finalizer, class_instance_shape_id, class_instance_type_id, class_layout,
    class_storage, layout, object_type_id, seq_access,
};
use crate::obj_from_bits;
use std::sync::atomic::{AtomicU64, Ordering};

/// The class payload word that holds the plan: a scalar, no reference, left
/// alone by class teardown.
pub(crate) const CLASS_LIFECYCLE_PLAN_WORD: usize = layout::CLASS_NATIVE_NEXT_WORD + 1;

const PRESENT: u64 = 1;
/// Some MRO member declares `__del__`.
const FINALIZER: u64 = 1 << 1;
/// Instances are `TYPE_ID_OBJECT` with shape `Plain` and no native prefix or
/// intrinsic layout owner; their fields are the contiguous words from offset
/// 0, and the dictionary word is the tail.
const PLAIN: u64 = 1 << 2;
/// Instances are never GC members (the root `object` alone, as in CPython).
/// An identity fact: set when the class seals and kept by every recompute.
const UNTRACKED: u64 = 1 << 3;
/// `object.__new__` may allocate an instance from the plan alone: `PLAIN`,
/// not abstract, not a poll-adapter class.
const INLINE_ALLOC: u64 = 1 << 4;
/// The exact class is a poll adapter, whose instances need a native poll
/// payload: an identity fact that refuses `INLINE_ALLOC`.
const POLL_ADAPTER: u64 = 1 << 5;
const IDENTITY_FACTS: u64 = UNTRACKED | POLL_ADAPTER;
/// The allocator size class of a plain instance (header and payload).
const SIZE_CLASS_SHIFT: u32 = 16;
const FIELD_WORDS_SHIFT: u32 = 32;

/// The facts about a class's identity that no mutation changes, given when
/// it seals.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClassIdentity {
    /// The builtin `object`.
    pub(crate) root_object: bool,
    /// One of the builtin poll-adapter classes.
    pub(crate) poll_adapter: bool,
}

/// One class's lifecycle plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClassLifecyclePlan(u64);

impl ClassLifecyclePlan {
    const ABSENT: Self = Self(0);

    #[inline(always)]
    pub(crate) fn is_present(self) -> bool {
        self.0 & PRESENT != 0
    }

    #[inline(always)]
    pub(crate) fn has_finalizer(self) -> bool {
        self.0 & FINALIZER != 0
    }

    /// A present plan for plain instances with no finalizer: the release of
    /// an instance that holds no pointer frees it in place.
    #[inline(always)]
    pub(crate) fn frees_trivially(self) -> bool {
        self.0 & (PRESENT | PLAIN | FINALIZER) == PRESENT | PLAIN
    }

    #[inline(always)]
    pub(crate) fn instances_untracked(self) -> bool {
        self.0 & UNTRACKED != 0
    }

    /// `object.__new__` allocates from the plan alone.
    #[inline(always)]
    pub(crate) fn allocates_inline(self) -> bool {
        self.0 & INLINE_ALLOC != 0
    }

    /// The field word count of a plain class.
    #[inline(always)]
    pub(crate) fn field_words(self) -> usize {
        (self.0 >> FIELD_WORDS_SHIFT) as usize
    }

    /// The payload bytes of a plain instance's layout: its fields and the
    /// dictionary word.
    #[inline(always)]
    pub(crate) fn payload_bytes(self) -> usize {
        (self.field_words() + 1) * std::mem::size_of::<u64>()
    }

    /// The allocator size class of a plain instance, nonzero when the plan
    /// allocates inline.
    #[inline(always)]
    pub(crate) fn size_class(self) -> u16 {
        (self.0 >> SIZE_CLASS_SHIFT) as u16
    }

    /// Apply the identity facts to a computed plan.
    fn with_identity(self, identity: u64) -> Self {
        if !self.is_present() {
            return Self::ABSENT;
        }
        let mut word = self.0 | identity;
        if word & PLAIN == 0 || word & POLL_ADAPTER != 0 {
            word &= !INLINE_ALLOC;
        }
        Self(word)
    }
}

#[inline(always)]
unsafe fn plan_word<'a>(class: *mut u8) -> &'a AtomicU64 {
    unsafe {
        &*(class.add(CLASS_LIFECYCLE_PLAN_WORD * std::mem::size_of::<u64>()) as *const AtomicU64)
    }
}

/// The published plan of `class`, a live `TYPE_ID_TYPE` payload.
#[inline(always)]
pub(crate) unsafe fn plan(class: *mut u8) -> ClassLifecyclePlan {
    let plan = ClassLifecyclePlan(unsafe { plan_word(class) }.load(Ordering::Acquire));
    #[cfg(debug_assertions)]
    if plan.is_present() {
        assert_eq!(
            plan,
            unsafe { compute(class) }.with_identity(plan.0 & IDENTITY_FACTS),
            "stale class lifecycle plan"
        );
    }
    plan
}

/// Whether some member of `class`'s MRO declares `__del__`. Reads header
/// flags only: no allocation, no Python code.
pub(crate) unsafe fn mro_declares_finalizer(class: *mut u8) -> bool {
    unsafe {
        if class_header_declares_finalizer(class) {
            return true;
        }
        obj_from_bits(layout::class_mro_bits(class))
            .as_ptr()
            .and_then(|mro| {
                seq_access::with_immutable_tuple_slice(mro, |mro| {
                    mro.iter().skip(1).any(|&bits| {
                        obj_from_bits(bits)
                            .as_ptr()
                            .is_some_and(|base| class_header_declares_finalizer(base))
                    })
                })
            })
            .unwrap_or(false)
    }
}

/// The field word count when `class`'s instances are plain, else None.
unsafe fn plain_field_words(class: *mut u8) -> Option<usize> {
    use class_storage::{ClassDeclaration, class_declares};
    unsafe {
        if class_instance_type_id(class) != TYPE_ID_OBJECT
            || class_instance_shape_id(class) != ObjectShapeId::Plain
            || super::class_reserved_layout_prefix(class) != 0
        {
            return None;
        }
        let mro = obj_from_bits(layout::class_mro_bits(class)).as_ptr()?;
        let intrinsic = seq_access::with_immutable_tuple_slice(mro, |mro| {
            mro.iter().any(|&bits| {
                obj_from_bits(bits).as_ptr().is_some_and(|base| {
                    object_type_id(base) == TYPE_ID_TYPE
                        && (class_declares(base, ClassDeclaration::IntrinsicLayout)
                            || class_declares(base, ClassDeclaration::IntValue)
                            || class_declares(base, ClassDeclaration::FloatValue))
                })
            })
        })?;
        if intrinsic {
            return None;
        }
        let size = layout::class_cached_layout_size(class)?;
        // Class teardown and cycle clearing empty the record slot.
        let record = layout::class_field_layout_bits(class);
        obj_from_bits(record).as_ptr()?;
        let (_, rows) = class_layout::projection_parts(record);
        let rows = obj_from_bits(rows).as_ptr()?;
        let words = seq_access::with_immutable_tuple_slice(rows, |rows| {
            let mut count = 0usize;
            for (index, row) in rows.as_chunks::<3>().0.iter().enumerate() {
                let field = class_layout::decode_row(row);
                if field.kind.is_intrinsic() || field.offset != index * std::mem::size_of::<u64>() {
                    return None;
                }
                count = index + 1;
            }
            Some(count)
        })??;
        // The fields, then the one dictionary word.
        (size == (words + 1) * std::mem::size_of::<u64>()).then_some(words)
    }
}

/// The plan `class`'s current facts give, without identity facts.
pub(crate) unsafe fn compute(class: *mut u8) -> ClassLifecyclePlan {
    unsafe {
        if object_type_id(class) != TYPE_ID_TYPE || !class_definition_is_finished(class) {
            return ClassLifecyclePlan::ABSENT;
        }
        let mut word = PRESENT;
        if mro_declares_finalizer(class) {
            word |= FINALIZER;
        }
        if let Some(words) = plain_field_words(class) {
            word |= PLAIN | ((words as u64) << FIELD_WORDS_SHIFT);
            let total =
                std::mem::size_of::<super::MoltHeader>() + (words + 1) * std::mem::size_of::<u64>();
            let size_class = super::size_class_for(total);
            if size_class != 0 && !class_storage::class_is_abstract(class) {
                word |= INLINE_ALLOC | (u64::from(size_class) << SIZE_CLASS_SHIFT);
            }
        }
        ClassLifecyclePlan(word)
    }
}

/// Publish the plan of a class that just sealed, with its identity facts.
pub(crate) unsafe fn publish_sealed(class: *mut u8, identity: ClassIdentity) {
    unsafe {
        let facts = if identity.root_object { UNTRACKED } else { 0 }
            | if identity.poll_adapter {
                POLL_ADAPTER
            } else {
                0
            };
        let plan = compute(class).with_identity(facts);
        plan_word(class).store(plan.0, Ordering::Release);
    }
}

/// Recompute the plan of `class` and of every descendant: each one's plan
/// reads its whole MRO.
pub(crate) unsafe fn refresh_family(class: *mut u8) {
    unsafe {
        refresh(class);
        super::class_subclasses::for_each_descendant(class, |subclass| refresh(subclass));
    }
}

/// Recompute the plan of a class a mutation reached, keeping its identity
/// facts. A class that has not sealed keeps no plan.
pub(crate) unsafe fn refresh(class: *mut u8) {
    unsafe {
        let word = plan_word(class);
        let identity = word.load(Ordering::Acquire) & IDENTITY_FACTS;
        let plan = compute(class).with_identity(identity);
        word.store(plan.0, Ordering::Release);
    }
}
