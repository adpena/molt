//! One builder-parametric generator for a proven instance field read and
//! store (`obj.x`, `obj.x = v` at a fixed offset; call_anatomy.md 4.5a). The
//! frontend emits a `load`/`store` at a fixed offset only where it proved the
//! receiver's class and that class's layout; the meaning of that access lives
//! here once, and a backend spells only the shared control and word
//! primitives (`FieldSlotBuilder`), as the list item generators do.
//!
//! ```text
//!   if !is_ptr(obj): get -> None, set -> nothing  ; a failed allocation's
//!                                                 ; sentinel, its exception
//!                                                 ; already pending
//!   p = unbox_ptr(obj)
//!   attempt {
//!     require type_id(p) == TYPE_ID_OBJECT        ; the field base is p
//!     flags = flags(p)
//!     require !(flags & HAS_INSTANCE_DICT)        ; the inline word owns it
//!     get: w = load [p + offset]
//!          if w is a pointer to a TYPE_ID_OBJECT without an inline class:
//!            cold                                 ; the missing word: class
//!                                                 ; fallback or AttributeError
//!          retain w
//!     set: if is_ptr(v) and !(flags & HAS_PTRS): atomic or HAS_PTRS
//!          old = load [p + offset]; retain v; store v
//!          if flags & HAS_PTRS: release old       ; else old owns nothing
//!   } else {
//!     cold(p)                                     ; the runtime entry
//!   }
//! ```
//!
//! Invariants:
//!
//! 1. A `TYPE_ID_OBJECT` instance keeps its fields at the object address: only
//!    a builtin-payload subclass (another type id) moves its field base.
//! 2. Once an instance has a dictionary, an inferred field lives in it, not in
//!    the inline word (molt-runtime `field_storage::resolve`). The runtime
//!    sets `HEADER_FLAG_HAS_INSTANCE_DICT` when it publishes the dictionary and
//!    never clears it, so a clear bit proves the inline word is the field.
//!    `HAS_PTRS` cannot stand in: any pointer store sets it.
//! 3. An empty field holds the runtime's missing singleton, an immortal
//!    class-0 `TYPE_ID_OBJECT` (`is_missing_bits`). A read sends every
//!    `TYPE_ID_OBJECT` word without an inline class to `cold`, a superset of
//!    the missing words; `cold` runs class fallback or raises.
//! 4. Every writer of a pointer into a field marks the object `HAS_PTRS`
//!    first (the runtime's `object_field_set_ptr_raw`, field init and delete,
//!    dictionary publication, and this store, with a relaxed atomic or). So
//!    while the bit is clear every field holds an immediate or the immortal
//!    missing word, and a store skips the old word's release. A store retains
//!    the new word before it overwrites the slot and releases the old one
//!    after, so a finalizer the release runs sees the new value.
//! 5. Under the GIL no other thread runs compiled code or replaces a field
//!    between the load and the retain. A free-threaded build takes `cold`.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::frame_record::FrameBuilder;
use crate::list_item::ListLocking;
use crate::word::WordComparison;
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_AUX_KIND_OFFSET, HEADER_AUX_OFFSET,
    HEADER_CLASS_WORD_BITS_MASK, HEADER_FLAG_HAS_INSTANCE_DICT, HEADER_FLAG_HAS_PTRS,
    HEADER_FLAGS_OFFSET, HEADER_TYPE_ID_OFFSET, POINTER_MASK, QNAN, TAG_NONE, TYPE_ID_OBJECT,
};

/// The aux kind sits in the high 16 bits of the 64-bit header word at the
/// flags offset (little-endian, every Molt target).
const AUX_KIND_SHIFT: i32 = 8 * (HEADER_AUX_KIND_OFFSET - HEADER_FLAGS_OFFSET);
const _: () = assert!(AUX_KIND_SHIFT == 48);

/// The one primitive a field store adds to the frame builder.
pub trait FieldSlotBuilder: FrameBuilder {
    /// OR `flag` into the 32-bit header flags of the object at `pointer`, as
    /// one relaxed atomic read-modify-write (native header flags are atomic:
    /// a scheduler worker may update a task's flags without the GIL).
    fn or_header_flag(&mut self, pointer: Self::Word, flag: u32);
}

fn compare_immediate<B: FrameBuilder>(
    b: &mut B,
    comparison: WordComparison,
    value: B::Word,
    immediate: u64,
) -> B::Predicate {
    let immediate = b.constant(immediate);
    b.compare(comparison, value, immediate)
}

fn is_clear<B: FrameBuilder>(b: &mut B, word: B::Word, mask: u64) -> B::Predicate {
    let bits = b.and(word, mask);
    compare_immediate(b, WordComparison::Equal, bits, 0)
}

/// The receiver tests every access shares, inside an `attempt`: the header
/// flags word when the inline slot owns the field.
fn admit_receiver<B: FrameBuilder>(b: &mut B, pointer: B::Word) -> B::Word {
    let kind = b.load_header_u32(pointer, HEADER_TYPE_ID_OFFSET);
    let is_object = compare_immediate(b, WordComparison::Equal, kind, u64::from(TYPE_ID_OBJECT));
    b.require(is_object);
    let flags = b.load_header_u32(pointer, HEADER_FLAGS_OFFSET);
    let no_dict = is_clear(b, flags, u64::from(HEADER_FLAG_HAS_INSTANCE_DICT));
    b.require(no_dict);
    flags
}

/// Inside an `attempt`: continue only when `word` is not a `TYPE_ID_OBJECT`
/// without an inline class word (invariant 3).
fn require_not_missing<B: FrameBuilder>(b: &mut B, word: B::Word) {
    let is_ptr = generate_has_tag(b, word, BoxTag::Ptr);
    b.if_then(is_ptr, |b| {
        let pointer = b.and(word, POINTER_MASK);
        let kind = b.load_header_u32(pointer, HEADER_TYPE_ID_OFFSET);
        let is_object =
            compare_immediate(b, WordComparison::Equal, kind, u64::from(TYPE_ID_OBJECT));
        b.if_then(is_object, |b| {
            let header = b.load_word(pointer, HEADER_FLAGS_OFFSET);
            let aux_kind = b.and(header, 0xFFFF << AUX_KIND_SHIFT);
            let class_inline = compare_immediate(
                b,
                WordComparison::Equal,
                aux_kind,
                u64::from(HEADER_AUX_KIND_CLASS_INLINE) << AUX_KIND_SHIFT,
            );
            b.require(class_inline);
            let class_word = b.load_word(pointer, HEADER_AUX_OFFSET);
            let class_bits = b.and(class_word, HEADER_CLASS_WORD_BITS_MASK);
            let has_class = compare_immediate(b, WordComparison::NotEqual, class_bits, 0);
            b.require(has_class);
        });
    });
}

/// `object.<field at offset>` as an owned word: the inline field (retained),
/// None for a non-pointer receiver, or what `cold(pointer)` returns, the
/// runtime entry that owns a dictionary-backed or empty field.
///
/// Requires: `object` is a borrowed boxed word whose class the frontend
/// proved, with the field at `offset`. Ensures: the result is owned by the
/// caller; the fast path runs no Python code and changes only the field
/// value's refcount.
pub fn generate_field_slot_get<B: FieldSlotBuilder>(
    b: &mut B,
    object: B::Word,
    offset: i32,
    locking: ListLocking,
    cold: impl FnOnce(&mut B, B::Word) -> B::Word,
) -> B::Word {
    let result = b.local();
    let none = b.constant(QNAN | TAG_NONE);
    b.set(result, none);
    let is_ptr = generate_has_tag(b, object, BoxTag::Ptr);
    b.if_then(is_ptr, |b| {
        let pointer = b.and(object, POINTER_MASK);
        if locking == ListLocking::FreeThreaded {
            let value = cold(b, pointer);
            b.set(result, value);
            return;
        }
        b.attempt(
            |b| {
                admit_receiver(b, pointer);
                let value = b.load_word(pointer, offset);
                require_not_missing(b, value);
                b.retain(value);
                b.set(result, value);
            },
            |b| {
                let value = cold(b, pointer);
                b.set(result, value);
            },
        );
    });
    b.get(result)
}

/// `object.<field at offset> = value`; the result is always None (the
/// runtime entry `cold(pointer)` reports failure through the exception
/// state).
///
/// Requires: `object` and `value` are borrowed boxed words; the frontend
/// proved `object`'s class and the field at `offset`. Ensures: on the fast
/// path the field holds its own reference to `value` and dropped the one it
/// held (invariant 4).
pub fn generate_field_slot_set<B: FieldSlotBuilder>(
    b: &mut B,
    object: B::Word,
    offset: i32,
    value: B::Word,
    locking: ListLocking,
    cold: impl FnOnce(&mut B, B::Word),
) {
    let is_ptr = generate_has_tag(b, object, BoxTag::Ptr);
    b.if_then(is_ptr, |b| {
        let pointer = b.and(object, POINTER_MASK);
        if locking == ListLocking::FreeThreaded {
            cold(b, pointer);
            return;
        }
        b.attempt(
            |b| {
                let flags = admit_receiver(b, pointer);
                let value_is_ptr = generate_has_tag(b, value, BoxTag::Ptr);
                b.if_then(value_is_ptr, |b| {
                    let unmarked = is_clear(b, flags, u64::from(HEADER_FLAG_HAS_PTRS));
                    b.if_then(unmarked, |b| {
                        b.or_header_flag(pointer, HEADER_FLAG_HAS_PTRS)
                    });
                });
                let old = b.load_word(pointer, offset);
                b.retain(value);
                b.store_word(pointer, offset, value);
                // Without HAS_PTRS the old word owns nothing (invariant 4).
                let ptrs = b.and(flags, u64::from(HEADER_FLAG_HAS_PTRS));
                let old_may_own = compare_immediate(b, WordComparison::NotEqual, ptrs, 0);
                b.if_then(old_may_own, |b| b.release(old));
            },
            |b| cold(b, pointer),
        );
    });
}

#[cfg(test)]
mod tests;
