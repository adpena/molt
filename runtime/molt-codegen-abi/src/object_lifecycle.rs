//! The class lifecycle plan word and the inline allocation and terminal free
//! of a plain instance (call_anatomy.md 4.5, "Implementation (object
//! lifecycle, HF-202)"). The runtime computes the plan when a class seals and
//! recomputes it at every class mutation (molt-runtime
//! `object/lifecycle_plan.rs`); this module owns its layout, the header image
//! of a planned allocation and the two generators every backend lowers through
//! its frame builder, as `field_slot` is.
//!
//! ```text
//!   allocate(class):
//!     require profile word == 0                  ; counters need the runtime
//!     plan = load [class + CLASS_LIFECYCLE_PLAN_OFFSET]
//!     require plan has INLINE_ALLOC and UNTRACKED
//!     base = allocator(object bytes)             ; the runtime charges limits
//!     require base != 0
//!     store header words; payload words = 0      ; UNTRACKED: no fields
//!     retain class; result = box(base + HEADER)
//!   else: cold                                   ; the runtime allocation
//!
//!   release_terminal(bits):                      ; the count read 1
//!     p = unbox(bits)
//!     require type_id(p) == TYPE_ID_OBJECT
//!     require flags(p) & OBSERVED == 0, aux kind CLASS_INLINE, owned class
//!     plan = load [class + CLASS_LIFECYCLE_PLAN_OFFSET]
//!     require plan frees trivially and UNTRACKED
//!     require no terminal release active on the thread, profile word == 0
//!     free(p - HEADER, object bytes); release class
//!   else: cold                                   ; molt_dec_ref_obj
//! ```
//!
//! Invariants:
//!
//! 1. The plan word equals the runtime's computation for the class's current
//!    facts whenever compiled code runs (the runtime rewrites it inside every
//!    class mutation), and a zero word means no plan.
//! 2. A planned allocation's header image is [`planned_header_words`]; the
//!    runtime's own planned allocation writes the same words.
//! 3. A free requires everything the runtime's terminal fast path requires
//!    (`free_plain_leaf`): no observer can see the death, and the bounded
//!    release would run it now. An UNTRACKED object was never accounted by
//!    the collector, so it has no GC state to retire.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::frame_record::FrameBuilder;
use crate::list_item::ListLocking;
use crate::word::WordComparison;
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_AUX_KIND_OFFSET, HEADER_AUX_OFFSET,
    HEADER_CLASS_WORD_BITS_MASK, HEADER_CLASS_WORD_BORROWED, HEADER_FLAG_DEALLOCATING,
    HEADER_FLAG_GC_ACCOUNTED, HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAG_HAS_PTRS,
    HEADER_FLAG_HAS_WEAKREF, HEADER_FLAG_SCOPED, HEADER_FLAG_SPAWN_RETAIN, HEADER_FLAGS_OFFSET,
    HEADER_SIZE_BYTES, HEADER_TYPE_ID_OFFSET, POINTER_MASK, QNAN, TAG_PTR, TYPE_ID_OBJECT,
    TYPE_ID_TYPE,
};

/// Byte offset of the lifecycle plan word in a `TYPE_ID_TYPE` payload.
pub const CLASS_LIFECYCLE_PLAN_OFFSET: i32 = 16 * 8;

/// The plan was computed (a sealed class). Zero means no plan.
pub const PLAN_PRESENT: u64 = 1;
/// Some MRO member declares `__del__`.
pub const PLAN_FINALIZER: u64 = 1 << 1;
/// Instances are `TYPE_ID_OBJECT` with shape `Plain`, no native prefix or
/// intrinsic layout owner; their fields are the contiguous words from offset
/// 0 and the dictionary word is the last word of the allocation.
pub const PLAN_PLAIN: u64 = 1 << 2;
/// Instances are never GC members (the root `object` alone, as in CPython).
pub const PLAN_UNTRACKED: u64 = 1 << 3;
/// `object.__new__` may allocate an instance from the plan alone.
pub const PLAN_INLINE_ALLOC: u64 = 1 << 4;
/// The exact class is a poll adapter (refuses `PLAN_INLINE_ALLOC`).
pub const PLAN_POLL_ADAPTER: u64 = 1 << 5;
/// The facts set once when the class seals and kept by every recompute.
pub const PLAN_IDENTITY_FACTS: u64 = PLAN_UNTRACKED | PLAN_POLL_ADAPTER;
/// Bits 16..32: the allocator size class of a plain instance.
pub const PLAN_SIZE_CLASS_SHIFT: u32 = 16;
/// Bits 32..48: the field word count of a plain class.
pub const PLAN_FIELD_WORDS_SHIFT: u32 = 32;
/// Bits 48..64: the object bytes of the size class (header and payload).
pub const PLAN_OBJECT_BYTES_SHIFT: u32 = 48;

/// The flags whose presence makes a death observable or the storage not the
/// allocator's: the release takes the runtime.
pub const OBSERVED_DEATH_FLAGS: u32 = HEADER_FLAG_HAS_WEAKREF
    | HEADER_FLAG_HAS_ABI_VIEW
    | HEADER_FLAG_SPAWN_RETAIN
    | HEADER_FLAG_SCOPED
    | HEADER_FLAG_HAS_PTRS
    | HEADER_FLAG_DEALLOCATING;

/// The first two header words of a planned allocation: the type id with an
/// owner count of 1, then the published flags (`gc_head` names the GC head
/// flag), the size class and the inline class aux kind. The third word is the
/// owned class word.
pub const fn planned_header_words(size_class: u16, flags: u32) -> [u64; 2] {
    [
        TYPE_ID_OBJECT as u64 | (1u64 << 32),
        flags as u64 | ((size_class as u64) << 32) | ((HEADER_AUX_KIND_CLASS_INLINE as u64) << 48),
    ]
}

const _: () = {
    assert!(HEADER_TYPE_ID_OFFSET == -HEADER_SIZE_BYTES);
    assert!(HEADER_FLAGS_OFFSET == HEADER_TYPE_ID_OFFSET + 8);
    assert!(HEADER_AUX_KIND_OFFSET == HEADER_FLAGS_OFFSET + 6);
    assert!(HEADER_AUX_OFFSET == HEADER_FLAGS_OFFSET + 8);
    assert!(PLAN_OBJECT_BYTES_SHIFT + 16 == 64);
};

/// The aux kind sits in the high 16 bits of the 64-bit header word at the
/// flags offset (little-endian, every Molt target).
const AUX_KIND_SHIFT: u32 = 8 * (HEADER_AUX_KIND_OFFSET - HEADER_FLAGS_OFFSET) as u32;

fn compare_immediate<B: FrameBuilder>(
    b: &mut B,
    comparison: WordComparison,
    value: B::Word,
    immediate: u64,
) -> B::Predicate {
    let immediate = b.constant(immediate);
    b.compare(comparison, value, immediate)
}

/// `value & mask == expected`.
fn masked_equals<B: FrameBuilder>(
    b: &mut B,
    value: B::Word,
    mask: u64,
    expected: u64,
) -> B::Predicate {
    let masked = b.and(value, mask);
    compare_immediate(b, WordComparison::Equal, masked, expected)
}

/// The object bytes a plan word records.
fn object_bytes<B: FrameBuilder>(b: &mut B, plan: B::Word) -> B::Word {
    // A logical shift: the field is the top 16 bits.
    let shifted = b.shift_right_arithmetic(plan, PLAN_OBJECT_BYTES_SHIFT);
    b.and(shifted, 0xFFFF)
}

/// What a plan must say for compiled code to allocate inline: present, plain
/// with a size class, not abstract or a poll adapter, and (until compiled code
/// tracks GC members, S4) untracked.
const INLINE_ALLOC_MASK: u64 = PLAN_PRESENT | PLAN_INLINE_ALLOC | PLAN_UNTRACKED;
/// What a plan must say for compiled code to free inline: present, plain, no
/// finalizer, untracked.
const INLINE_FREE_MASK: u64 = PLAN_PRESENT | PLAN_PLAIN | PLAN_FINALIZER | PLAN_UNTRACKED;
const INLINE_FREE_EXPECTED: u64 = PLAN_PRESENT | PLAN_PLAIN | PLAN_UNTRACKED;

/// The words a lifecycle generator reads besides its operand.
#[derive(Clone, Copy)]
pub struct LifecycleWords<W> {
    /// The thread record's profile-enabled word (`ThreadRecordWord::ProfileEnabled`).
    pub profile_enabled: W,
}

/// Where an allocation site finds its class's plan (call_anatomy.md 4.5,
/// object lifecycle step 7).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanSource {
    /// A class the program may still mutate: load and test its plan word.
    Guarded,
    /// A class whose plan the closed-world analysis proved: no plan load and
    /// no plan test.
    Proven(u64),
}

/// Allocate an instance of the class whose boxed bits are `class_bits` from
/// its lifecycle plan, owned. `allocate(bytes)` calls the runtime allocator
/// entry and answers the base address or 0; `cold` is the runtime's own
/// allocation of the class, which owns every refusal and error.
pub fn generate_object_alloc<B: FrameBuilder>(
    b: &mut B,
    class_bits: B::Word,
    plan_source: PlanSource,
    words: LifecycleWords<B::Word>,
    locking: ListLocking,
    allocate: impl FnOnce(&mut B, B::Word) -> B::Word,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    let result = b.local();
    let refused = matches!(
        plan_source,
        PlanSource::Proven(plan) if plan & INLINE_ALLOC_MASK != INLINE_ALLOC_MASK
    );
    if locking == ListLocking::FreeThreaded || refused {
        let value = cold(b);
        b.set(result, value);
        return b.get(result);
    }
    b.attempt(
        |b| {
            let quiet = compare_immediate(b, WordComparison::Equal, words.profile_enabled, 0);
            b.require(quiet);
            let plan = match plan_source {
                PlanSource::Proven(plan) => b.constant(plan),
                PlanSource::Guarded => {
                    let pointer = generate_has_tag(b, class_bits, BoxTag::Ptr);
                    b.require(pointer);
                    let class = b.and(class_bits, POINTER_MASK);
                    let is_class = b.load_header_u32(class, HEADER_TYPE_ID_OFFSET);
                    let is_class =
                        compare_immediate(b, WordComparison::Equal, is_class, TYPE_ID_TYPE as u64);
                    b.require(is_class);
                    let plan = b.load_word(class, CLASS_LIFECYCLE_PLAN_OFFSET);
                    let planned = masked_equals(b, plan, INLINE_ALLOC_MASK, INLINE_ALLOC_MASK);
                    b.require(planned);
                    plan
                }
            };
            let bytes = object_bytes(b, plan);
            let base = allocate(b, bytes);
            let allocated = compare_immediate(b, WordComparison::NotEqual, base, 0);
            b.require(allocated);
            // Header: no GC head (UNTRACKED), so the flags are zero.
            let size_class = b.shift_right_arithmetic(plan, PLAN_SIZE_CLASS_SHIFT);
            let size_class = b.and(size_class, 0xFFFF);
            let [first, second] = planned_header_words(0, 0);
            let first = b.constant(first);
            b.store_word(base, 0, first);
            let size_class = b.shift_left(size_class, 32);
            let second = b.or(size_class, second);
            b.store_word(base, 8, second);
            // The owned class word: the class's boxed bits, no borrowed tag.
            b.store_word(base, 16, class_bits);
            // Payload: an UNTRACKED class has no fields, so every word,
            // including the dictionary word, is empty.
            let payload = b.add_immediate(bytes, -(HEADER_SIZE_BYTES as i64));
            let count = b.shift_right_arithmetic(payload, 3);
            let data = b.add_immediate(base, HEADER_SIZE_BYTES as i64);
            b.counted_loop(count, false, |b, index| {
                let offset = b.shift_left(index, 3);
                let address = b.add(data, offset);
                let zero = b.constant(0);
                b.store_word(address, 0, zero);
            });
            b.retain(class_bits);
            let boxed = b.or(data, QNAN | TAG_PTR);
            b.set(result, boxed);
        },
        |b| {
            let value = cold(b);
            b.set(result, value);
        },
    );
    b.get(result)
}

/// Free the object whose boxed bits are `bits` after its count read 1, when
/// nothing can observe the death; otherwise `cold`, the runtime release
/// (`molt_dec_ref_obj`). `free(base, bytes)` calls the runtime allocator
/// entry. `terminal_release` is the thread record's terminal-release word,
/// nonzero while a terminal release runs on the thread.
pub fn generate_release_terminal<B: FrameBuilder>(
    b: &mut B,
    bits: B::Word,
    words: LifecycleWords<B::Word>,
    terminal_release: B::Word,
    locking: ListLocking,
    free: impl FnOnce(&mut B, B::Word, B::Word),
    cold: impl FnOnce(&mut B),
) {
    if locking == ListLocking::FreeThreaded {
        cold(b);
        return;
    }
    b.attempt(
        |b| {
            let pointer = b.and(bits, POINTER_MASK);
            let type_id = b.load_header_u32(pointer, HEADER_TYPE_ID_OFFSET);
            let object =
                compare_immediate(b, WordComparison::Equal, type_id, TYPE_ID_OBJECT as u64);
            b.require(object);
            let header = b.load_word(pointer, HEADER_FLAGS_OFFSET);
            // Flags in the low 32 bits, the aux kind in the high 16.
            let flag_mask = (OBSERVED_DEATH_FLAGS | HEADER_FLAG_GC_ACCOUNTED) as u64
                | (0xFFFF_u64 << AUX_KIND_SHIFT);
            let unobserved = masked_equals(
                b,
                header,
                flag_mask,
                (HEADER_AUX_KIND_CLASS_INLINE as u64) << AUX_KIND_SHIFT,
            );
            b.require(unobserved);
            let class_word = b.load_word(pointer, HEADER_AUX_OFFSET);
            let owned = masked_equals(b, class_word, HEADER_CLASS_WORD_BORROWED, 0);
            b.require(owned);
            // The class word holds the class's boxed bits (a class object).
            let class_bits = b.and(class_word, HEADER_CLASS_WORD_BITS_MASK);
            let has_class = compare_immediate(b, WordComparison::NotEqual, class_bits, 0);
            b.require(has_class);
            let class = b.and(class_bits, POINTER_MASK);
            let plan = b.load_word(class, CLASS_LIFECYCLE_PLAN_OFFSET);
            let trivial = masked_equals(b, plan, INLINE_FREE_MASK, INLINE_FREE_EXPECTED);
            b.require(trivial);
            let outermost = compare_immediate(b, WordComparison::Equal, terminal_release, 0);
            b.require(outermost);
            let quiet = compare_immediate(b, WordComparison::Equal, words.profile_enabled, 0);
            b.require(quiet);
            let bytes = object_bytes(b, plan);
            let base = b.add_immediate(pointer, -(HEADER_SIZE_BYTES as i64));
            free(b, base, bytes);
            b.release(class_bits);
        },
        |b| cold(b),
    );
}

#[cfg(test)]
mod tests;
