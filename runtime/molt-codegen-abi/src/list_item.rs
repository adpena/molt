//! One builder-parametric generator for a compiled sequence item read and a
//! list item store (`xs[i]`, `t[i]`, `xs[i] = v`; call_anatomy.md 4.5a). The
//! meaning lives here once; a backend spells only the shared control and word
//! primitives (`FrameBuilder`), as the method site generators reuse the frame
//! builder.
//!
//! ```text
//!   attempt {
//!     require is_ptr(seq); require is_inline_int(index)
//!     p = unbox_ptr(seq)
//!     require !(flags(p) & HAS_ABI_VIEW)    ; no C view mirrors the storage
//!     require aux_kind(p) <= CLASS_INLINE and class bits(aux(p)) == 0
//!                                           ; no class edge: an exact builtin
//!     kind = type_id(p); require kind is one the access admits
//!                        (ItemContainers: lists, tuple, or either; a store
//!                        admits lists only, a proof drops the dead tests)
//!     LIST, LIST_INT: s = load [p]          ; the list's MoltVec
//!                     len = load [s + LIST_STORAGE_LEN_OFFSET]
//!                     data = load [s + LIST_STORAGE_DATA_OFFSET]
//!     TUPLE:          len = load [p + TUPLE_STORAGE_LEN_OFFSET]
//!                     data = p + TUPLE_STORAGE_ITEMS_OFFSET
//!     i = index; if i < 0 { i += len }
//!     require i <u len
//!     slot = data + 8 i
//!     LIST, TUPLE: get loads and retains the word; a list set retains the new
//!                  word, stores it and releases the old one
//!     LIST_INT:    get boxes the raw inline int; set stores an inline int's
//!                  payload (any other value takes cold, which converts)
//!   } else {
//!     cold()                                ; the runtime entry, unchanged
//!   }
//! ```
//!
//! Invariants:
//!
//! 1. An exact builtin list or tuple never carries a class edge: the runtime
//!    allocates both with an inline class word of zero
//!    (`ObjectAuxPreselection::ClassInline`), and a subclass instance stores
//!    its class there or has another type id. So a list or tuple type id, an
//!    aux kind of `NONE` or `CLASS_INLINE` and zero class bits admit exactly
//!    the exact builtins. A subclass, or an object with a sidecar, takes
//!    `cold`, which keeps `__getitem__`/`__setitem__` overrides.
//! 2. Every list kind's storage is a `MoltVec` with its data pointer and
//!    length at `LIST_STORAGE_DATA_OFFSET` / `LIST_STORAGE_LEN_OFFSET`; a tuple
//!    keeps a 64-bit length word and its items inline, at
//!    `TUPLE_STORAGE_LEN_OFFSET` / `TUPLE_STORAGE_ITEMS_OFFSET`. Both are
//!    asserted at compile time beside their structs. The storage kind is a
//!    heap fact read from the type id at each access: a store can promote a
//!    compact list to generic words, so no type proof selects it. A compact
//!    int list holds only inline ints; a compact bool list takes `cold`.
//! 3. Under the GIL the access runs without the list's mutation lock: no other
//!    thread runs compiled code, and nothing between the length load and the
//!    element access runs Python code. A free-threaded build takes `cold`.
//! 4. A store retains the new value before it overwrites the slot and
//!    releases the old one after, so a finalizer the release runs observes
//!    the list already holding the new value (CPython's `list_ass_item`). A
//!    tuple store is never admitted: the runtime raises its TypeError.
//! 5. Every other case (out of range, a bool, big-int or `__index__` index,
//!    another container) takes `cold`, which raises CPython's errors or runs
//!    the protocol.

use crate::boxed_word::{
    BoxTag, generate_box_inline_int, generate_has_tag, generate_lacks_tag,
    generate_unbox_inline_int,
};
use crate::frame_record::FrameBuilder;
use crate::word::WordComparison;
use crate::{
    HEADER_AUX_KIND_CLASS_INLINE, HEADER_AUX_KIND_OFFSET, HEADER_AUX_OFFSET,
    HEADER_CLASS_WORD_BITS_MASK, HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAGS_OFFSET,
    HEADER_TYPE_ID_OFFSET, LIST_STORAGE_DATA_OFFSET, LIST_STORAGE_LEN_OFFSET, POINTER_MASK,
    TUPLE_STORAGE_ITEMS_OFFSET, TUPLE_STORAGE_LEN_OFFSET, TYPE_ID_LIST, TYPE_ID_LIST_INT,
    TYPE_ID_TUPLE,
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

/// The exact builtins an item access admits, from the container's proven
/// TIR type. A choice never changes behavior: a container of another kind
/// takes `cold`. It only removes the tests a proof makes dead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemContainers {
    /// The generic and compact int lists: every store, and a read of a
    /// proven list.
    Lists,
    /// An exact tuple: a read of a proven tuple.
    Tuple,
    /// Either: a read of a container no type proves.
    ListsOrTuple,
}

/// A storage kind's admission: known by the container's proof, or tested.
enum When<P> {
    Always,
    Never,
    If(P),
}

fn when<B: FrameBuilder>(b: &mut B, condition: &When<B::Predicate>, body: impl FnOnce(&mut B)) {
    match *condition {
        When::Always => body(b),
        When::Never => {}
        When::If(predicate) => b.if_then(predicate, body),
    }
}

/// An admitted element of an exact list or tuple: its slot and its storage
/// kind.
struct Element<W, P> {
    slot: W,
    /// The slot holds a boxed word (a generic list or a tuple).
    boxed: When<P>,
    /// The slot holds a compact list's raw inline int.
    int: When<P>,
}

fn set_flag<B: FrameBuilder>(b: &mut B, flag: B::Local) {
    let one = b.constant(1);
    b.set(flag, one);
}

fn flag_is_set<B: FrameBuilder>(b: &mut B, flag: B::Local) -> B::Predicate {
    let value = b.get(flag);
    compare_immediate(b, WordComparison::NotEqual, value, 0)
}

/// A tuple's length and the address of its first item.
fn tuple_storage<B: FrameBuilder>(b: &mut B, pointer: B::Word) -> (B::Word, B::Word) {
    let len = b.load_word(pointer, TUPLE_STORAGE_LEN_OFFSET);
    let data = b.add_immediate(pointer, i64::from(TUPLE_STORAGE_ITEMS_OFFSET));
    (len, data)
}

/// A list's length and data pointer, from its `MoltVec`.
fn list_storage<B: FrameBuilder>(b: &mut B, pointer: B::Word) -> (B::Word, B::Word) {
    let storage = b.load_word(pointer, 0);
    let len = b.load_word(storage, LIST_STORAGE_LEN_OFFSET);
    let data = b.load_word(storage, LIST_STORAGE_DATA_OFFSET);
    (len, data)
}

/// Every test of the fast path, inside an `attempt`; a failed test has
/// branched to its cold path.
fn generate_element<B: FrameBuilder>(
    b: &mut B,
    sequence: B::Word,
    index: B::Word,
    containers: ItemContainers,
) -> Element<B::Word, B::Predicate> {
    let is_ptr = generate_has_tag(b, sequence, BoxTag::Ptr);
    b.require(is_ptr);
    let is_int = generate_has_tag(b, index, BoxTag::Int);
    b.require(is_int);
    let raw = generate_unbox_inline_int(b, index);
    let pointer = b.and(sequence, POINTER_MASK);
    let header = b.load_word(pointer, HEADER_FLAGS_OFFSET);
    require_clear(b, header, HEADER_ADMISSION_MASK);
    let class_word = b.load_word(pointer, HEADER_AUX_OFFSET);
    require_clear(b, class_word, HEADER_CLASS_WORD_BITS_MASK);
    let kind = b.load_header_u32(pointer, HEADER_TYPE_ID_OFFSET);
    let kind_is = |b: &mut B, type_id: u32| {
        compare_immediate(b, WordComparison::Equal, kind, u64::from(type_id))
    };
    // Admit the kind before any storage load: only a list's payload word is a
    // MoltVec pointer, and only a tuple keeps its items inline.
    let (len, data, boxed, int) = match containers {
        ItemContainers::Tuple => {
            let is_tuple = kind_is(b, TYPE_ID_TUPLE);
            b.require(is_tuple);
            let (len, data) = tuple_storage(b, pointer);
            (len, data, When::Always, When::Never)
        }
        ItemContainers::Lists | ItemContainers::ListsOrTuple => {
            let is_list_word = kind_is(b, TYPE_ID_LIST);
            let is_list_int = kind_is(b, TYPE_ID_LIST_INT);
            let [boxed, admitted, len, data] = [(); 4].map(|()| b.local());
            let zero = b.constant(0);
            b.set(boxed, zero);
            b.set(admitted, zero);
            b.if_then(is_list_word, |b| set_flag(b, boxed));
            let admit = |b: &mut B, (length, items): (B::Word, B::Word)| {
                b.set(len, length);
                b.set(data, items);
                set_flag(b, admitted);
            };
            for list_kind in [is_list_word, is_list_int] {
                b.if_then(list_kind, |b| {
                    let storage = list_storage(b, pointer);
                    admit(b, storage);
                });
            }
            if containers == ItemContainers::ListsOrTuple {
                let is_tuple = kind_is(b, TYPE_ID_TUPLE);
                b.if_then(is_tuple, |b| {
                    set_flag(b, boxed);
                    let storage = tuple_storage(b, pointer);
                    admit(b, storage);
                });
            }
            let is_admitted = flag_is_set(b, admitted);
            b.require(is_admitted);
            let len = b.get(len);
            let data = b.get(data);
            let boxed = flag_is_set(b, boxed);
            (len, data, When::If(boxed), When::If(is_list_int))
        }
    };
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
    // Every admitted kind holds 8-byte elements.
    let offset = b.shift_left(position, 3);
    let slot = b.add(data, offset);
    Element { slot, boxed, int }
}

/// `list[index]` or `tuple[index]` as an owned word: the element (retained when
/// it is a boxed word), or what `cold` returns, the runtime entry that owns
/// every other case and every error.
///
/// Requires: `sequence` and `index` are borrowed boxed words. Ensures: the
/// result is owned by the caller; the fast path runs no Python code and
/// changes only the element's refcount.
pub fn generate_list_item_get<B: FrameBuilder>(
    b: &mut B,
    sequence: B::Word,
    index: B::Word,
    containers: ItemContainers,
    locking: ListLocking,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    if locking == ListLocking::FreeThreaded {
        return cold(b);
    }
    let result = b.local();
    b.attempt(
        |b| {
            let element = generate_element(b, sequence, index, containers);
            when(b, &element.boxed, |b| {
                let item = b.load_word(element.slot, 0);
                b.retain(item);
                b.set(result, item);
            });
            when(b, &element.int, |b| {
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
            let element = generate_element(b, list, index, ItemContainers::Lists);
            // A compact int list stores only an inline int; any other value
            // converts the storage in the runtime.
            let is_inline = generate_has_tag(b, value, BoxTag::Int);
            let stored = b.local();
            let zero = b.constant(0);
            b.set(stored, zero);
            when(b, &element.boxed, |b| {
                let old = b.load_word(element.slot, 0);
                // A generic list keeps an exact count of its heap-pointer
                // items (molt-runtime `TrackedVecBox::heap_edge_count`).
                // The fast path admits only a store that keeps it: old and
                // new both pointers, or both not. A store that changes the
                // count takes `cold`, which maintains it.
                let old_is_ptr = generate_has_tag(b, old, BoxTag::Ptr);
                b.if_then(old_is_ptr, |b| {
                    let value_is_ptr = generate_has_tag(b, value, BoxTag::Ptr);
                    b.require(value_is_ptr);
                });
                let old_is_not_ptr = generate_lacks_tag(b, old, BoxTag::Ptr);
                b.if_then(old_is_not_ptr, |b| {
                    let value_is_not_ptr = generate_lacks_tag(b, value, BoxTag::Ptr);
                    b.require(value_is_not_ptr);
                });
                b.retain(value);
                b.store_word(element.slot, 0, value);
                b.release(old);
                let one = b.constant(1);
                b.set(stored, one);
            });
            let When::If(is_int) = element.int else {
                unreachable!("a list store tests the compact int kind")
            };
            let int_store = b.both(is_int, is_inline);
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
