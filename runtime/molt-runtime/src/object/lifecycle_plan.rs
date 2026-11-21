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

// The plan word's layout is `molt_codegen_abi::object_lifecycle`'s, which
// compiled allocation and release read inline.
use molt_codegen_abi::object_lifecycle::{
    PLAN_FIELD_WORDS_SHIFT as FIELD_WORDS_SHIFT, PLAN_FINALIZER as FINALIZER,
    PLAN_IDENTITY_FACTS as IDENTITY_FACTS, PLAN_INLINE_ALLOC as INLINE_ALLOC, PLAN_PLAIN as PLAIN,
    PLAN_POLL_ADAPTER as POLL_ADAPTER, PLAN_PRESENT as PRESENT,
    PLAN_SIZE_CLASS_SHIFT as SIZE_CLASS_SHIFT, PLAN_UNTRACKED as UNTRACKED,
};
const _: () = assert!(
    CLASS_LIFECYCLE_PLAN_WORD * 8
        == molt_codegen_abi::object_lifecycle::CLASS_LIFECYCLE_PLAN_OFFSET as usize
);

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
        ((self.0 >> FIELD_WORDS_SHIFT) & 0xFFFF) as usize
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
            let (rows, _) = rows.as_chunks::<3>();
            for (index, row) in rows.iter().enumerate() {
                let field = class_layout::decode_row(row);
                if field.kind.is_intrinsic() || field.offset != index * std::mem::size_of::<u64>() {
                    return None;
                }
            }
            Some(rows.len())
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
            // The one formula, which a proven allocation site also compiles in.
            word |= molt_codegen_abi::object_lifecycle::plain_instance_plan(
                words,
                !class_storage::class_is_abstract(class),
            );
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
        // Compiled `object()` allocates from the constant root plan.
        assert!(
            !identity.root_object || plan.0 == molt_codegen_abi::object_lifecycle::ROOT_OBJECT_PLAN,
            "the builtin object's plan diverged from ROOT_OBJECT_PLAN"
        );
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
