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
//!     plan = load [class + CLASS_LIFECYCLE_PLAN_OFFSET]   ; or a proven constant
//!     require plan has INLINE_ALLOC
//!     fields: require the missing singleton exists
//!     tracked (not UNTRACKED): require a GC record, room in generation 0
//!       and count 0 below threshold 0       ; else the runtime schedules
//!     base = allocator(head + object bytes)  ; the runtime charges limits
//!     require base != 0
//!     store head?, header words; payload = 0; fields = missing
//!     tracked: head = (len << 3) | 1; members[len] = (object, ordinal);
//!              len, ordinal and count 0 advance
//!     retain class; result = box(object)
//!   else: cold                                   ; the runtime allocation
//!
//!   release_terminal(bits):                      ; the count read 1
//!     p = unbox(bits)
//!     require type_id(p) == TYPE_ID_OBJECT
//!     require flags(p) & OBSERVED == 0, aux kind CLASS_INLINE, owned class
//!     plan = load [class + CLASS_LIFECYCLE_PLAN_OFFSET]
//!     require plan is plain without a finalizer
//!     require no terminal release active on the thread, profile word == 0
//!     accounted: require a head, a GC record and no retirement watch;
//!       a tracked head swap-removes its member (the moved member's head
//!       follows); count 0 falls by one, never below zero
//!     free(p); release class
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
//!    release would run it now.
//! 4. The GC record (`GC_RECORD_*`) is the head of the bound runtime's
//!    collector state; each isolate's thread record lends its own. The layout
//!    is the 64-bit one: a free-threaded runtime, or one whose pointers are
//!    narrower, lends no record and keeps tracking in the runtime. A member
//!    list with no room, a count at its threshold and an armed retirement
//!    watch are the runtime's, which grows, schedules and counts.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::frame_record::FrameBuilder;
use crate::list_item::ListLocking;
use crate::word::WordComparison;
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_AUX_KIND_OFFSET, HEADER_AUX_OFFSET,
    HEADER_CLASS_WORD_BITS_MASK, HEADER_CLASS_WORD_BORROWED, HEADER_FLAG_DEALLOCATING,
    HEADER_FLAG_GC_ACCOUNTED, HEADER_FLAG_GC_HEAD, HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAG_HAS_PTRS,
    HEADER_FLAG_HAS_WEAKREF, HEADER_FLAG_SCOPED, HEADER_FLAG_SPAWN_RETAIN, HEADER_FLAGS_OFFSET,
    HEADER_SIZE_BYTES, HEADER_TYPE_ID_OFFSET, POINTER_MASK, QNAN, TAG_PTR, TYPE_ID_FOREIGN,
    TYPE_ID_OBJECT, TYPE_ID_TYPE,
};

/// The allocator size classes: index 0 is the oversized sentinel.
pub const SIZE_CLASSES: usize = 81;

/// Predefined size classes (in bytes) for object allocations.
/// Index 0 is reserved for oversized allocations whose exact size lives in the
/// immutable aux sidecar.
/// Indices 1..=N map to common allocation sizes up to 64 KB.
pub const SIZE_CLASS_TABLE: [usize; SIZE_CLASSES] = [
    0, // 0: sentinel / oversized
    8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 128, 144, 160, 176, 192, 208,
    224, 240, 256, 288, 320, 352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 832, 896, 960, 1024,
    1152, 1280, 1408, 1536, 1664, 1792, 1920, 2048, 2304, 2560, 2816, 3072, 3328, 3584, 3840, 4096,
    4608, 5120, 5632, 6144, 6656, 7168, 7680, 8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360,
    16384, 20480, 24576, 28672, 32768, 40960, 49152, 57344, 65536,
];

/// Map an allocation size (in bytes) to a `u16` size-class index.
///
/// Returns 0 (oversized sentinel) when `size` exceeds the largest class.
/// Otherwise returns the smallest class index whose value >= `size`.
pub const fn size_class_for(size: usize) -> u16 {
    // Linear scan is fine: the table has < 90 entries and this is called
    // once per allocation, not on the hot refcount path.
    let mut index = 1;
    while index < SIZE_CLASSES {
        if SIZE_CLASS_TABLE[index] >= size {
            return index as u16;
        }
        index += 1;
    }
    0 // oversized
}

/// The plan of a class whose instances are plain: `field_words` contiguous
/// field words, then the dictionary word; no finalizer and no identity facts.
/// It is the one formula: the runtime publishes it for such a class when the
/// class seals (molt-runtime `lifecycle_plan::compute`), and an allocation
/// whose class layout the frontend proved compiles it in (`PlanSource::Proven`).
/// `may_inline` is false for an abstract class. Instances too large for a size
/// class (or for the plan's 16-bit fields) never allocate inline.
pub const fn plain_instance_plan(field_words: usize, may_inline: bool) -> u64 {
    let word =
        PLAN_PRESENT | PLAN_PLAIN | (((field_words as u64) & 0xFFFF) << PLAN_FIELD_WORDS_SHIFT);
    if !may_inline || field_words >= 1 << 16 {
        return word;
    }
    let size_class = size_class_for(HEADER_SIZE_BYTES as usize + (field_words + 1) * 8);
    let object_bytes = SIZE_CLASS_TABLE[size_class as usize] as u64;
    if size_class == 0 || object_bytes >= 1 << 16 {
        return word;
    }
    word | PLAN_INLINE_ALLOC
        | ((size_class as u64) << PLAN_SIZE_CLASS_SHIFT)
        | (object_bytes << PLAN_OBJECT_BYTES_SHIFT)
}

/// The plan of the builtin `object`, which no code can change (its class is
/// immutable): a 32-byte untracked instance with no fields. The runtime
/// asserts it when the class seals; compiled `object()` allocates from it.
pub const ROOT_OBJECT_PLAN: u64 = plain_instance_plan(0, true) | PLAN_UNTRACKED;

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

// The GC record: the head of a runtime's `GcRuntimeState` (molt-runtime
// `object/gc.rs`), whose address the thread record lends for the bound runtime
// (`THREAD_RECORD_GC_RECORD_OFFSET`). The runtime asserts every offset.
/// u64 x 3: the generation thresholds; generation 0 first.
pub const GC_RECORD_THRESHOLD0_OFFSET: i32 = 0;
/// u64 x 3: the generation counts; count 0 is GC allocations minus frees.
pub const GC_RECORD_COUNT0_OFFSET: i32 = 24;
/// One member list (`ptr`, `len`, `cap`) per generation, permanent last.
pub const GC_RECORD_MEMBERS_OFFSET: i32 = 48;
/// Bytes of one member list.
pub const GC_MEMBER_LIST_BYTES: i32 = 24;
/// The generation lists, the permanent one included.
pub const GC_GENERATION_LISTS: i32 = 4;
/// u64: the next allocation ordinal.
pub const GC_RECORD_NEXT_ORDINAL_OFFSET: i32 =
    GC_RECORD_MEMBERS_OFFSET + GC_GENERATION_LISTS * GC_MEMBER_LIST_BYTES;
/// u64: nonzero while a retirement watch is armed (untracks go cold).
pub const GC_RECORD_WATCH_ARMED_OFFSET: i32 = GC_RECORD_NEXT_ORDINAL_OFFSET + 8;
/// Bytes of one member: the object's data address, then its ordinal.
pub const GC_MEMBER_BYTES: i32 = 16;
/// Bytes of the GC head before a tracked object's header.
pub const GC_HEAD_BYTES: i32 = 8;
// The GC head word: bit 0 tracked, bits 1..=2 generation, bits 3.. position.
pub const GC_HEAD_TRACKED: u64 = 1;
pub const GC_HEAD_GENERATION_SHIFT: u32 = 1;
pub const GC_HEAD_GENERATION_MASK: u64 = 0b11;
pub const GC_HEAD_POSITION_SHIFT: u32 = 3;

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

/// What a plan must say for compiled code to allocate inline: present and
/// inline-allocated (plain, sealed, not abstract or a poll adapter, a small
/// size class).
const INLINE_ALLOC_MASK: u64 = PLAN_PRESENT | PLAN_INLINE_ALLOC;
/// What a plan must say for compiled code to free inline: present, plain, no
/// finalizer.
const INLINE_FREE_MASK: u64 = PLAN_PRESENT | PLAN_PLAIN | PLAN_FINALIZER;
const INLINE_FREE_EXPECTED: u64 = PLAN_PRESENT | PLAN_PLAIN;
// `PLAN_UNTRACKED` is the GC head's size, so `(plan & UNTRACKED) ^ UNTRACKED`
// is the head bytes of an instance.
const _: () = assert!(PLAN_UNTRACKED == GC_HEAD_BYTES as u64);
/// The header flags of a tracked planned allocation: a head, and the count the
/// admission adds.
pub const TRACKED_HEADER_FLAGS: u32 = HEADER_FLAG_GC_HEAD | HEADER_FLAG_GC_ACCOUNTED;

/// The words a lifecycle generator reads besides its operand: thread-record
/// words of the bound runtime (`ThreadRecordWord`).
#[derive(Clone, Copy)]
pub struct LifecycleWords<W> {
    /// 1 when the process collects runtime profile counters (`ProfileEnabled`).
    pub profile_enabled: W,
    /// The bound runtime's GC record, or 0 (`GcRecord`).
    pub gc_record: W,
    /// The address of the missing singleton's slot, or 0 (`MissingSlot`).
    pub missing_slot: W,
}

/// `index * 24`, the byte offset of a member list.
fn list_offset<B: FrameBuilder>(b: &mut B, index: B::Word) -> B::Word {
    let sixteen = b.shift_left(index, 4);
    let eight = b.shift_left(index, 3);
    b.add(sixteen, eight)
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
/// its lifecycle plan, owned, admitting it to generation 0 when its class's
/// instances are GC members, as the runtime's admission does (the member's
/// head, its entry and ordinal, and count 0). `allocate(bytes)` calls the
/// runtime allocator entry and answers the base address or 0; `cold` is the
/// runtime's own allocation of the class, which owns every refusal and error,
/// including a full member list, a count at its threshold (the runtime then
/// schedules a collection) and an armed retirement watch.
pub fn generate_object_alloc<B: FrameBuilder>(
    b: &mut B,
    class_bits: B::Word,
    plan_source: PlanSource,
    words: LifecycleWords<B::Word>,
    locking: ListLocking,
    allocate: impl FnOnce(&mut B, B::Word) -> B::Word,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    use WordComparison::*;
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
            let quiet = compare_immediate(b, Equal, words.profile_enabled, 0);
            b.require(quiet);
            // The class operand is a class object (a proven root class word
            // is zero until the builtin classes exist).
            let pointer = generate_has_tag(b, class_bits, BoxTag::Ptr);
            b.require(pointer);
            let plan = match plan_source {
                PlanSource::Proven(plan) => b.constant(plan),
                PlanSource::Guarded => {
                    let class = b.and(class_bits, POINTER_MASK);
                    let is_class = b.load_header_u32(class, HEADER_TYPE_ID_OFFSET);
                    let is_class = compare_immediate(b, Equal, is_class, TYPE_ID_TYPE as u64);
                    b.require(is_class);
                    let plan = b.load_word(class, CLASS_LIFECYCLE_PLAN_OFFSET);
                    let planned = masked_equals(b, plan, INLINE_ALLOC_MASK, INLINE_ALLOC_MASK);
                    b.require(planned);
                    plan
                }
            };
            // The instance's fields hold the missing word: its slot must be
            // filled already.
            let fields = b.shift_right_arithmetic(plan, PLAN_FIELD_WORDS_SHIFT);
            let fields = b.and(fields, 0xFFFF);
            let missing = b.local();
            let zero = b.constant(0);
            b.set(missing, zero);
            let has_fields = compare_immediate(b, NotEqual, fields, 0);
            b.if_then(has_fields, |b| {
                let lent = compare_immediate(b, NotEqual, words.missing_slot, 0);
                b.require(lent);
                let singleton = b.load_word(words.missing_slot, 0);
                let created = compare_immediate(b, NotEqual, singleton, 0);
                b.require(created);
                let bits = b.or(singleton, QNAN | TAG_PTR);
                b.set(missing, bits);
            });
            // Admission is decided before anything is allocated, so a refusal
            // leaves no state behind.
            let untracked = b.and(plan, PLAN_UNTRACKED);
            let tracked = compare_immediate(b, Equal, untracked, 0);
            let head_bytes = b.local();
            let none = b.constant(0);
            b.set(head_bytes, none);
            b.if_then(tracked, |b| {
                let eight = b.constant(GC_HEAD_BYTES as u64);
                b.set(head_bytes, eight);
            });
            let head_bytes = b.get(head_bytes);
            let gc = words.gc_record;
            b.if_then(tracked, |b| {
                let lent = compare_immediate(b, NotEqual, gc, 0);
                b.require(lent);
                let length = b.load_word(gc, GC_RECORD_MEMBERS_OFFSET + 8);
                let capacity = b.load_word(gc, GC_RECORD_MEMBERS_OFFSET + 16);
                let room = b.compare(UnsignedLessThan, length, capacity);
                b.require(room);
                let count = b.load_word(gc, GC_RECORD_COUNT0_OFFSET);
                let threshold = b.load_word(gc, GC_RECORD_THRESHOLD0_OFFSET);
                let below = b.compare(UnsignedLessThan, count, threshold);
                b.require(below);
            });
            let bytes = object_bytes(b, plan);
            let bytes = b.add(bytes, head_bytes);
            let base = allocate(b, bytes);
            let allocated = compare_immediate(b, NotEqual, base, 0);
            b.require(allocated);
            let header = b.add(base, head_bytes);
            let data = b.add_immediate(header, HEADER_SIZE_BYTES as i64);
            // Header.
            let size_class = b.shift_right_arithmetic(plan, PLAN_SIZE_CLASS_SHIFT);
            let size_class = b.and(size_class, 0xFFFF);
            let [first, second] = planned_header_words(0, 0);
            let first = b.constant(first);
            b.store_word(header, 0, first);
            let size_class = b.shift_left(size_class, 32);
            let second = b.or(size_class, second);
            let flags = b.local();
            b.set(flags, second);
            b.if_then(tracked, |b| {
                let current = b.get(flags);
                let marked = b.or(current, TRACKED_HEADER_FLAGS as u64);
                b.set(flags, marked);
            });
            let second = b.get(flags);
            b.store_word(header, 8, second);
            // The owned class word: the class's boxed bits, no borrowed tag.
            b.store_word(header, 16, class_bits);
            // Payload: every word empty, the fields then the missing word.
            let object = object_bytes(b, plan);
            let payload = b.add_immediate(object, -(HEADER_SIZE_BYTES as i64));
            let count = b.shift_right_arithmetic(payload, 3);
            b.counted_loop(count, false, |b, index| {
                let offset = b.shift_left(index, 3);
                let address = b.add(data, offset);
                let zero = b.constant(0);
                b.store_word(address, 0, zero);
            });
            let missing = b.get(missing);
            b.counted_loop(fields, false, |b, index| {
                let offset = b.shift_left(index, 3);
                let address = b.add(data, offset);
                b.store_word(address, 0, missing);
            });
            // Admission: head, entry, ordinal, length and count 0.
            b.if_then(tracked, |b| {
                let length = b.load_word(gc, GC_RECORD_MEMBERS_OFFSET + 8);
                let members = b.load_word(gc, GC_RECORD_MEMBERS_OFFSET);
                let ordinal = b.load_word(gc, GC_RECORD_NEXT_ORDINAL_OFFSET);
                let position = b.shift_left(length, GC_HEAD_POSITION_SHIFT);
                let head = b.or(position, GC_HEAD_TRACKED);
                b.store_word(base, 0, head);
                let entry = b.shift_left(length, 4);
                let entry = b.add(members, entry);
                b.store_word(entry, 0, data);
                b.store_word(entry, 8, ordinal);
                let length = b.add_immediate(length, 1);
                b.store_word(gc, GC_RECORD_MEMBERS_OFFSET + 8, length);
                let ordinal = b.add_immediate(ordinal, 1);
                b.store_word(gc, GC_RECORD_NEXT_ORDINAL_OFFSET, ordinal);
                let count = b.load_word(gc, GC_RECORD_COUNT0_OFFSET);
                let count = b.add_immediate(count, 1);
                b.store_word(gc, GC_RECORD_COUNT0_OFFSET, count);
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
/// (`molt_dec_ref_obj`). A GC member leaves its list as the runtime's untrack
/// does (swap-remove, the moved member's head rewritten) and count 0 falls by
/// one. `free(object)` calls the runtime allocator entry, which reads the
/// allocation's extent and head from the header.
/// `terminal_release` is the thread record's terminal-release word, nonzero
/// while a terminal release runs on the thread.
pub fn generate_release_terminal<B: FrameBuilder>(
    b: &mut B,
    bits: B::Word,
    words: LifecycleWords<B::Word>,
    terminal_release: B::Word,
    locking: ListLocking,
    free: impl FnOnce(&mut B, B::Word),
    cold: impl FnOnce(&mut B),
) {
    use WordComparison::*;
    if locking == ListLocking::FreeThreaded {
        cold(b);
        return;
    }
    b.attempt(
        |b| {
            let pointer = b.and(bits, POINTER_MASK);
            let type_id = b.load_header_u32(pointer, HEADER_TYPE_ID_OFFSET);
            let object = compare_immediate(b, Equal, type_id, TYPE_ID_OBJECT as u64);
            b.require(object);
            let header = b.load_word(pointer, HEADER_FLAGS_OFFSET);
            // Flags in the low 32 bits, the aux kind in the high 16.
            let flag_mask = OBSERVED_DEATH_FLAGS as u64 | (0xFFFF_u64 << AUX_KIND_SHIFT);
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
            let has_class = compare_immediate(b, NotEqual, class_bits, 0);
            b.require(has_class);
            let class = b.and(class_bits, POINTER_MASK);
            let plan = b.load_word(class, CLASS_LIFECYCLE_PLAN_OFFSET);
            let trivial = masked_equals(b, plan, INLINE_FREE_MASK, INLINE_FREE_EXPECTED);
            b.require(trivial);
            let outermost = compare_immediate(b, Equal, terminal_release, 0);
            b.require(outermost);
            let quiet = compare_immediate(b, Equal, words.profile_enabled, 0);
            b.require(quiet);
            // A GC member's state: an accounted allocation has a head.
            let accounted = b.and(header, HEADER_FLAG_GC_ACCOUNTED as u64);
            let accounted = compare_immediate(b, NotEqual, accounted, 0);
            let gc = words.gc_record;
            b.if_then(accounted, |b| {
                let headed = b.and(header, HEADER_FLAG_GC_HEAD as u64);
                let headed = compare_immediate(b, NotEqual, headed, 0);
                b.require(headed);
                let lent = compare_immediate(b, NotEqual, gc, 0);
                b.require(lent);
                let watching = b.load_word(gc, GC_RECORD_WATCH_ARMED_OFFSET);
                let quiet = compare_immediate(b, Equal, watching, 0);
                b.require(quiet);
                let head_at =
                    b.add_immediate(pointer, -((HEADER_SIZE_BYTES + GC_HEAD_BYTES) as i64));
                let head = b.load_word(head_at, 0);
                let tracked = masked_equals(b, head, GC_HEAD_TRACKED, GC_HEAD_TRACKED);
                b.if_then(tracked, |b| {
                    let generation = b.shift_right_arithmetic(head, GC_HEAD_GENERATION_SHIFT);
                    let generation = b.and(generation, GC_HEAD_GENERATION_MASK);
                    let position = b.shift_right_arithmetic(head, GC_HEAD_POSITION_SHIFT);
                    let list = list_offset(b, generation);
                    let list = b.add(gc, list);
                    let length = b.load_word(list, GC_RECORD_MEMBERS_OFFSET + 8);
                    let inside = b.compare(UnsignedLessThan, position, length);
                    b.require(inside);
                    let members = b.load_word(list, GC_RECORD_MEMBERS_OFFSET);
                    let last = b.add_immediate(length, -1);
                    let moves = b.compare(NotEqual, position, last);
                    b.if_then(moves, |b| {
                        let from = b.shift_left(last, 4);
                        let from = b.add(members, from);
                        let moved_slot = b.load_word(from, 0);
                        let moved_ordinal = b.load_word(from, 8);
                        let to = b.shift_left(position, 4);
                        let to = b.add(members, to);
                        b.store_word(to, 0, moved_slot);
                        b.store_word(to, 8, moved_ordinal);
                        // The moved member's head names its new position.
                        let generation_bits = b.shift_left(generation, GC_HEAD_GENERATION_SHIFT);
                        let position_bits = b.shift_left(position, GC_HEAD_POSITION_SHIFT);
                        let moved_head = b.or(position_bits, GC_HEAD_TRACKED);
                        let moved_head = b.add(moved_head, generation_bits);
                        let moved_head_at = b.add_immediate(
                            moved_slot,
                            -((HEADER_SIZE_BYTES + GC_HEAD_BYTES) as i64),
                        );
                        b.store_word(moved_head_at, 0, moved_head);
                    });
                    b.store_word(list, GC_RECORD_MEMBERS_OFFSET + 8, last);
                    let cleared = b.constant(0);
                    b.store_word(head_at, 0, cleared);
                });
                // Count 0 is GC allocations minus frees, never below zero.
                let count = b.load_word(gc, GC_RECORD_COUNT0_OFFSET);
                let positive = compare_immediate(b, NotEqual, count, 0);
                b.if_then(positive, |b| {
                    let count = b.add_immediate(count, -1);
                    b.store_word(gc, GC_RECORD_COUNT0_OFFSET, count);
                });
            });
            free(b, pointer);
            b.release(class_bits);
        },
        |b| cold(b),
    );
}

/// `type(value)`, owned: the class an object names in its inline class word
/// (retained), else what `cold` answers, the runtime's `type(value)`
/// (`molt_type_of`), which owns immediates, builtin layouts without a class
/// word, sidecars, native objects and failures. The runtime reads the same
/// word (`object_class_bits`): the bits under the class tag, when they name a
/// class object.
pub fn generate_type_of<B: FrameBuilder>(
    b: &mut B,
    value: B::Word,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    use WordComparison::*;
    let result = b.local();
    b.attempt(
        |b| {
            let pointer = generate_has_tag(b, value, BoxTag::Ptr);
            b.require(pointer);
            let object = b.and(value, POINTER_MASK);
            let header = b.load_word(object, HEADER_FLAGS_OFFSET);
            let aux_kind = b.and(header, 0xFFFF << AUX_KIND_SHIFT);
            let inline = compare_immediate(
                b,
                Equal,
                aux_kind,
                (HEADER_AUX_KIND_CLASS_INLINE as u64) << AUX_KIND_SHIFT,
            );
            b.require(inline);
            // A native object's type is its native type's wrapper.
            let type_id = b.load_header_u32(object, HEADER_TYPE_ID_OFFSET);
            let native = compare_immediate(b, NotEqual, type_id, TYPE_ID_FOREIGN as u64);
            b.require(native);
            let class_word = b.load_word(object, HEADER_AUX_OFFSET);
            let class_bits = b.and(class_word, HEADER_CLASS_WORD_BITS_MASK);
            let class_pointer = generate_has_tag(b, class_bits, BoxTag::Ptr);
            b.require(class_pointer);
            let class = b.and(class_bits, POINTER_MASK);
            let class_kind = b.load_header_u32(class, HEADER_TYPE_ID_OFFSET);
            let is_class = compare_immediate(b, Equal, class_kind, TYPE_ID_TYPE as u64);
            b.require(is_class);
            b.retain(class_bits);
            b.set(result, class_bits);
        },
        |b| {
            let value = cold(b);
            b.set(result, value);
        },
    );
    b.get(result)
}

/// The builtin-type tag of `object` (`BUILTIN_TYPE` operand; the frontend's
/// `BUILTIN_TYPE_TAGS`, the runtime's `builtin_type_bits`).
pub const BUILTIN_TAG_OBJECT: i64 = 100;

/// `BUILTIN_TYPE(tag)`, owned: the builtin `object` class the thread record
/// lends (`ThreadRecordWord::ObjectClass`, retained) when `tag` is the boxed
/// `object` tag, else what `cold` answers (`molt_builtin_type`). A constant
/// tag folds the test away.
pub fn generate_builtin_type<B: FrameBuilder>(
    b: &mut B,
    tag: B::Word,
    object_class: B::Word,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    use WordComparison::*;
    let result = b.local();
    b.attempt(
        |b| {
            let object_tag = compare_immediate(
                b,
                Equal,
                tag,
                crate::box_int_bits(BUILTIN_TAG_OBJECT) as u64,
            );
            b.require(object_tag);
            // Zero until the builtin classes exist.
            let lent = compare_immediate(b, NotEqual, object_class, 0);
            b.require(lent);
            b.retain(object_class);
            b.set(result, object_class);
        },
        |b| {
            let value = cold(b);
            b.set(result, value);
        },
    );
    b.get(result)
}

#[cfg(test)]
mod tests;
