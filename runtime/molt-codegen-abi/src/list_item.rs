//! One builder-parametric generator for a compiled list item read and store
//! (`xs[i]`, `xs[i] = v`; call_anatomy.md 4.5a). The meaning lives here once;
//! a backend spells only the shared control and word primitives
//! (`FrameBuilder`), as the method site generators reuse the frame builder.
//!
//! ```text
//!   attempt {
//!     require is_ptr(list); require is_inline_int(index)
//!     p = unbox_ptr(list)
//!     kind = type_id(p); require kind is LIST or LIST_INT
//!     require !(flags(p) & HAS_ABI_VIEW)    ; no C view mirrors the storage
//!     require aux_kind(p) <= CLASS_INLINE and class bits(aux(p)) == 0
//!                                           ; no class edge: an exact list
//!     s = load [p]                          ; the list's MoltVec| aux_kind) == 0
//!                                           ; no C view, no class edge
//!     s = load [p]                          ; the list's MoltVec<u64>
//!     len = load [s + LIST_STORAGE_LEN_OFFSET]
//!     i = index; if i < 0 { i += len }
//!     require i <u len
//!     slot = load [s + LIST_STORAGE_DATA_OFFSET] + 8 i
//!     LIST:     get loads and retains the word; set retains the new word,
//!               stores it and releases the old one
//!     LIST_INT: get boxes the raw inline int; set stores an inline int's
//!               payload (any other value takes cold, which converts)
//!   } else {
//!     cold()                                ; the runtime entry, unchanged
//!   }
//! ```
//!
//! Invariants:
//!
//! 1. An exact builtin list never carries a class edge: a list is allocated
//!    with an inline class word of zero (`ObjectAuxPreselection::ClassInline`)
//!    and a subclass instance stores its class there, so a list type id, an
//!    aux kind of `NONE` or `CLASS_INLINE` and zero class bits admit exactly
//!    the exact lists. A subclass, or a list with a sidecar, takes `cold`,
//!    which keeps `__getitem__`/`__setitem__` overrides.
//! 2. Every list kind's storage is a `MoltVec` with its data pointer and
//!    length at `LIST_STORAGE_DATA_OFFSET` / `LIST_STORAGE_LEN_OFFSET`, asserted
//!    at compile time beside the struct. The storage kind is a heap fact read
//!    from the type id at each access: a store can promote a compact list to
//!    generic words, so no type proof selects it. A compact int list holds
//!    only inline ints; a compact bool list takes `cold`.
//! 3. Under the GIL the access runs without the list's mutation lock: no other
//!    thread runs compiled code, and nothing between the length load and the
//!    element access runs Python code. A free-threaded build takes `cold`.
//! 4. A store retains the new value before it overwrites the slot and
//!    releases the old one after, so a finalizer the release runs observes
//!    the list already holding the new value (CPython's `list_ass_item`).
//! 5. Every other case (out of range, a bool, big-int or `__index__` index,
//!    another container) takes `cold`, which raises CPython's errors or runs
//!    the protocol.

use crate::boxed_word::{
    BoxTag, generate_box_inline_int, generate_has_tag, generate_unbox_inline_int,
};
use crate::frame_record::FrameBuilder;
use crate::word::WordComparison;
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_AUX_KIND_OFFSET, HEADER_AUX_OFFSET,
    HEADER_CLASS_WORD_BITS_MASK, HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAGS_OFFSET,
    HEADER_TYPE_ID_OFFSET, LIST_STORAGE_DATA_OFFSET, LIST_STORAGE_LEN_OFFSET, POINTER_MASK,
    TYPE_ID_LIST, TYPE_ID_LIST_INT,
};

/// The 64-bit header word from the flags field holds `flags` in its low half
/// and `size_class | aux_kind << 16` in its high half (little-endian, every
/// Molt target). One load and one mask test a C view and an aux kind above
/// `CLASS_INLINE` (a state word or a sidecar).
const AUX_KIND_SHIFT: i32 = 8 * (HEADER_AUX_KIND_OFFSET - HEADER_FLAGS_OFFSET);
const HEADER_ADMISSION_MASK: u64 = HEADER_FLAG_HAS_ABI_VIEW as u64
    | ((0xFFFF & !(HEADER_AUX_KIND_CLASS_INLINE as u64)) << AUX_KIND_SHIFT);
const _: () = {
    // NONE and CLASS_INLINE are the two kinds an exact list can carry.
    assert!(crate::HEADER_AUX_KIND_NONE == 0 && HEADER_AUX_KIND_CLASS_INLINE == 1);
    assert!(AUX_KIND_SHIFT == 48);
};

/// The runtime's threading mode, from the build's refcount storage
/// (`MOLT_REFCOUNT_ATOMIC`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListLocking {
    /// The GIL serializes compiled code: no mutation lock (invariant 3).
    Gil,
    /// Another thread may resize the list: every access takes `cold`.
    FreeThreaded,
}

impl ListLocking {
    /// The mode of the runtime this build links (`MOLT_REFCOUNT_ATOMIC`).
    pub const BUILD: Self = if crate::MOLT_REFCOUNT_ATOMIC {
        Self::FreeThreaded
    } else {
        Self::Gil
    };
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

fn require_clear<B: FrameBuilder>(b: &mut B, word: B::Word, mask: u64) {
    let bits = b.and(word, mask);
    let clear = compare_immediate(b, WordComparison::Equal, bits, 0);
    b.require(clear);
}

/// An admitted element of an exact list: its slot and its storage kind.
struct Element<W, P> {
    slot: W,
    is_word: P,
    is_int: P,
}

/// Every test of the fast path, inside an `attempt`; a failed test has
/// branched to its cold path.
fn generate_element<B: FrameBuilder>(
    b: &mut B,
    list: B::Word,
    index: B::Word,
) -> Element<B::Word, B::Predicate> {
    let is_ptr = generate_has_tag(b, list, BoxTag::Ptr);
    b.require(is_ptr);
    let is_int = generate_has_tag(b, index, BoxTag::Int);
    b.require(is_int);
    let raw = generate_unbox_inline_int(b, index);
    let pointer = b.and(list, POINTER_MASK);
    let kind = b.load_header_u32(pointer, HEADER_TYPE_ID_OFFSET);
    let is_word = compare_immediate(b, WordComparison::Equal, kind, u64::from(TYPE_ID_LIST));
    let is_int = compare_immediate(b, WordComparison::Equal, kind, u64::from(TYPE_ID_LIST_INT));
    // Only a list kind's payload word is a MoltVec pointer: admit the kind
    // before the storage load.
    let listed = b.local();
    let zero = b.constant(0);
    b.set(listed, zero);
    for kind_matches in [is_word, is_int] {
        b.if_then(kind_matches, |b| {
            let one = b.constant(1);
            b.set(listed, one);
        });
    }
    let flag = b.get(listed);
    let is_list = compare_immediate(b, WordComparison::NotEqual, flag, 0);
    b.require(is_list);
    let header = b.load_word(pointer, HEADER_FLAGS_OFFSET);
    require_clear(b, header, HEADER_ADMISSION_MASK);
    let class_word = b.load_word(pointer, HEADER_AUX_OFFSET);
    require_clear(b, class_word, HEADER_CLASS_WORD_BITS_MASK);
    let storage = b.load_word(pointer, 0);
    let len = b.load_word(storage, LIST_STORAGE_LEN_OFFSET);
    let adjusted = b.local();
    b.set(adjusted, raw);
    let sign = b.shift_right_arithmetic(raw, 63);
    let negative = compare_immediate(b, WordComparison::NotEqual, sign, 0);
    b.if_then(negative, |b| {
        let wrapped = b.add(raw, len);
        b.set(adjusted, wrapped);
    });
    let position = b.get(adjusted);
    let in_bounds = b.compare(WordComparison::UnsignedLessThan, position, len);
    b.require(in_bounds);
    let data = b.load_word(storage, LIST_STORAGE_DATA_OFFSET);
    // Both admitted kinds hold 8-byte elements.
    let offset = b.shift_left(position, 3);
    let slot = b.add(data, offset);
    Element {
        slot,
        is_word,
        is_int,
    }
}

/// `list[index]` as an owned word: the element (retained when it is a boxed
/// word), or what `cold` returns, the runtime entry that owns every other
/// case and every error.
///
/// Requires: `list` and `index` are borrowed boxed words. Ensures: the
/// result is owned by the caller; the fast path runs no Python code and
/// changes only the element's refcount.
pub fn generate_list_item_get<B: FrameBuilder>(
    b: &mut B,
    list: B::Word,
    index: B::Word,
    locking: ListLocking,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    if locking == ListLocking::FreeThreaded {
        return cold(b);
    }
    let result = b.local();
    b.attempt(
        |b| {
            let element = generate_element(b, list, index);
            b.if_then(element.is_word, |b| {
                let item = b.load_word(element.slot, 0);
                b.retain(item);
                b.set(result, item);
            });
            b.if_then(element.is_int, |b| {
                // A compact int list admits only inline ints.
                let raw = b.load_word(element.slot, 0);
                let item = generate_box_inline_int(b, raw);
                b.set(result, item);
            });
        },
        |b| {
            let item = cold(b);
            b.set(result, item);
        },
    );
    b.get(result)
}

/// `list[index] = value`, returning `success` on the fast path (the word the
/// runtime entry `cold` calls returns when it stores: None, or the container)
/// or what `cold` returns.
///
/// Requires: all four words are borrowed. Ensures: on the fast path a generic
/// list holds its own reference to `value` and dropped the one it held
/// (invariant 4), and a compact int list holds an inline int's payload; a
/// value the compact list cannot hold takes `cold`, which converts it.
pub fn generate_list_item_set<B: FrameBuilder>(
    b: &mut B,
    list: B::Word,
    index: B::Word,
    value: B::Word,
    locking: ListLocking,
    success: B::Word,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    if locking == ListLocking::FreeThreaded {
        return cold(b);
    }
    let result = b.local();
    b.attempt(
        |b| {
            let element = generate_element(b, list, index);
            // A compact int list stores only an inline int; any other value
            // converts the storage in the runtime.
            let is_inline = generate_has_tag(b, value, BoxTag::Int);
            let stored = b.local();
            let zero = b.constant(0);
            b.set(stored, zero);
            b.if_then(element.is_word, |b| {
                let old = b.load_word(element.slot, 0);
                b.retain(value);
                b.store_word(element.slot, 0, value);
                b.release(old);
                let one = b.constant(1);
                b.set(stored, one);
            });
            let int_store = b.both(element.is_int, is_inline);
            b.if_then(int_store, |b| {
                let raw = generate_unbox_inline_int(b, value);
                b.store_word(element.slot, 0, raw);
                let one = b.constant(1);
                b.set(stored, one);
            });
            let flag = b.get(stored);
            let done = compare_immediate(b, WordComparison::NotEqual, flag, 0);
            b.require(done);
            b.set(result, success);
        },
        |b| {
            let stored = cold(b);
            b.set(result, stored);
        },
    );
    b.get(result)
}

#[cfg(test)]
mod tests;
