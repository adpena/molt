//! One builder-parametric generator for a synchronous frame's record push
//! and pop.
//!
//! A compiled synchronous function links its own frame record
//! (`FRAME_RECORD_*`, molt-runtime `builtins/frames/records.rs`) with plain
//! loads and stores on the thread record, as CPython's interpreter pushes an
//! `_PyInterpreterFrame` on its data stack. The runtime is called only off
//! the fast path:
//! * the push, when no invoker handed this code's function or the arena
//!   chunk has no room: `FrameBuilder::enter_slow` links an owned record;
//! * the pop, when the record's flags word is not zero (an observer attached
//!   a payload, the runtime owns the record, or it opened an arena chunk):
//!   `FrameBuilder::exit_slow`.
//!
//! A backend spells instructions only. Every layout constant, the handoff
//! test, the release order and the arena discipline live here.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::word::{WordBuilder, WordComparison};
use crate::{
    CODE_FRAME_SLOT_OFFSET, FRAME_HOME_BITS_OFFSET, FRAME_HOME_BYTES, FRAME_HOME_HOLDS_REFERENCE,
    FRAME_HOME_KIND_OFFSET, FRAME_HOME_UNBOUND, FRAME_LOCATION_OFFSET, FRAME_RECORD_CODE_OFFSET,
    FRAME_RECORD_FLAGS_OFFSET, FRAME_RECORD_FUNCTION_OFFSET, FRAME_RECORD_HEADER_BYTES,
    FRAME_RECORD_OBSERVER_OFFSET, FRAME_RECORD_PREV_OFFSET, FUNCTION_CODE_OFFSET,
    HEADER_TYPE_ID_OFFSET, POINTER_MASK, THREAD_RECORD_ARENA_LIMIT_OFFSET,
    THREAD_RECORD_ARENA_TOP_OFFSET, THREAD_RECORD_FRAME_TOP_OFFSET,
    THREAD_RECORD_PENDING_INVOCATION_OFFSET, TYPE_ID_FUNCTION,
};

/// The location word of a frame that has published no site
/// (molt-runtime `LocationId::ABSENT`, which asserts the value).
pub const FRAME_LOCATION_ABSENT: u64 = u64::MAX;

/// Bits 0..32 of a `trace_enter_slot` value: the frame's home count.
pub const FRAME_ENTRY_HOMES_MASK: i64 = 0xffff_ffff;
/// Set in a `trace_enter_slot` value when the frame clears its slots last to
/// first (CPython 3.14 and later; molt-runtime `FramePolicy::descending`).
pub const FRAME_ENTRY_DESCENDING: i64 = 1 << 32;

/// Above this many homes a pop releases them in one counted loop instead of
/// one inline release per home.
pub const FRAME_POP_INLINE_HOMES: u32 = 8;

/// How many homes a frame has: a compile-time count where the function's own
/// code is generated, a runtime word in a body shared by every function.
#[derive(Clone, Copy)]
pub enum HomeCount<W> {
    Constant(u32),
    Dynamic(W),
}

/// What a synchronous frame entry declares: the `trace_enter_slot` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameShape {
    pub homes: u32,
    pub descending: bool,
}

impl FrameShape {
    /// Decode a `trace_enter_slot` value.
    pub const fn from_entry_value(value: i64) -> Self {
        Self {
            homes: (value & FRAME_ENTRY_HOMES_MASK) as u32,
            descending: value & FRAME_ENTRY_DESCENDING != 0,
        }
    }

    pub const fn entry_value(self) -> i64 {
        self.homes as i64
            | if self.descending {
                FRAME_ENTRY_DESCENDING
            } else {
                0
            }
    }
}

/// Backend instruction spelling for frame transitions, extending the
/// backend's one `WordBuilder`. Control flow and layout are owned by the
/// generators below.
pub trait FrameBuilder: WordBuilder {
    /// A mutable word that survives control flow.
    type Local: Copy;

    fn local(&mut self) -> Self::Local;
    fn set(&mut self, local: Self::Local, value: Self::Word);
    fn get(&mut self, local: Self::Local) -> Self::Word;

    fn load_word(&mut self, address: Self::Word, offset: i32) -> Self::Word;
    fn store_word(&mut self, address: Self::Word, offset: i32, value: Self::Word);
    /// Load an object header's 32-bit field, zero-extended.
    fn load_header_u32(&mut self, pointer: Self::Word, offset: i32) -> Self::Word;
    fn add(&mut self, left: Self::Word, right: Self::Word) -> Self::Word;

    /// One owned-reference transition on a boxed word, through the backend's
    /// `generate_refcount` site.
    fn retain(&mut self, bits: Self::Word);
    fn release(&mut self, bits: Self::Word);

    /// `molt_frame_record_enter(code_slot)`: the linked record's base, or 0.
    fn enter_slow(&mut self, code_slot: Self::Word) -> Self::Word;
    /// `molt_frame_record_exit(base)`.
    fn exit_slow(&mut self, base: Self::Word);

    /// Run `fast`; a failed [`Self::require`] inside it continues in `slow`,
    /// which is cold. Both end in one continuation.
    fn attempt(&mut self, fast: impl FnOnce(&mut Self), slow: impl FnOnce(&mut Self));
    /// Inside [`Self::attempt`]'s `fast`: continue only when `condition`
    /// holds.
    fn require(&mut self, condition: Self::Predicate);
    fn if_then(&mut self, condition: Self::Predicate, then: impl FnOnce(&mut Self));
    /// Run `body` with `index` counting 0, 1, .. `count - 1`, or down from
    /// `count - 1` when `descending`. `count` may be zero. A code generator
    /// calls `body` once; an interpreter calls it per iteration.
    fn counted_loop(
        &mut self,
        count: Self::Word,
        descending: bool,
        body: impl FnMut(&mut Self, Self::Word),
    );
}

fn compare_immediate<B: WordBuilder>(
    b: &mut B,
    comparison: WordComparison,
    value: B::Word,
    immediate: u64,
) -> B::Predicate {
    let immediate = b.constant(immediate);
    b.compare(comparison, value, immediate)
}

fn home_bytes<B: FrameBuilder>(b: &mut B, homes: HomeCount<B::Word>) -> HomeCount<B::Word> {
    match homes {
        HomeCount::Constant(count) => HomeCount::Constant(count),
        // FRAME_HOME_BYTES is 16.
        HomeCount::Dynamic(count) => HomeCount::Dynamic(b.shift_left(count, 4)),
    }
}

const _: () = assert!(FRAME_HOME_BYTES == 16);

/// Generate a synchronous frame's entry.
///
/// Requires: `thread` is this invocation's thread record; `code_slot` is the
/// program slot id of the function's code; `homes` is the home count of that
/// code's frame plan. Runs once per invocation, before any home operation.
///
/// Ensures: the result local holds the base of a record linked as the
/// thread's executing frame, or zero when nothing was linked (an exception is
/// pending). On the inline path the record has unbound homes, an absent
/// location and a zero flags word, owns one reference to the handed function
/// and one to its code, and the handoff is consumed. A nonzero base with an
/// exception pending is a record without usable homes: the caller leaves
/// through its entry exception check and still pops.
pub fn generate_frame_push<B: FrameBuilder>(
    b: &mut B,
    thread: B::Word,
    code_slot: B::Word,
    homes: HomeCount<B::Word>,
) -> B::Local {
    use WordComparison::{Equal, UnsignedLessThan};
    let result = b.local();
    b.attempt(
        |b| {
            // The handoff: a function object whose code owns this slot.
            let function = b.load_word(thread, THREAD_RECORD_PENDING_INVOCATION_OFFSET);
            let is_object = generate_has_tag(b, function, BoxTag::Ptr);
            b.require(is_object);
            let function_ptr = b.and(function, POINTER_MASK);
            let type_id = b.load_header_u32(function_ptr, HEADER_TYPE_ID_OFFSET);
            let is_function = compare_immediate(b, Equal, type_id, u64::from(TYPE_ID_FUNCTION));
            b.require(is_function);
            let code = b.load_word(function_ptr, FUNCTION_CODE_OFFSET);
            let has_code = generate_has_tag(b, code, BoxTag::Ptr);
            b.require(has_code);
            let code_ptr = b.and(code, POINTER_MASK);
            // The code stores its slot id plus one.
            let owner = b.load_word(code_ptr, CODE_FRAME_SLOT_OFFSET);
            let expected = b.add_immediate(code_slot, 1);
            let owns_slot = b.compare(Equal, owner, expected);
            b.require(owns_slot);

            // Room in the current arena chunk.
            let start = b.load_word(thread, THREAD_RECORD_ARENA_TOP_OFFSET);
            let base = b.add_immediate(start, FRAME_RECORD_HEADER_BYTES);
            let end = match home_bytes(b, homes) {
                HomeCount::Constant(count) => {
                    b.add_immediate(base, i64::from(count) * FRAME_HOME_BYTES)
                }
                HomeCount::Dynamic(bytes) => b.add(base, bytes),
            };
            let limit = b.load_word(thread, THREAD_RECORD_ARENA_LIMIT_OFFSET);
            // end <= limit, as one unsigned compare. An empty arena (top and
            // limit both zero) never fits.
            let past_limit = b.add_immediate(limit, 1);
            let fits = b.compare(UnsignedLessThan, end, past_limit);
            b.require(fits);
            let zero = b.constant(0);
            b.store_word(thread, THREAD_RECORD_ARENA_TOP_OFFSET, end);

            let prev = b.load_word(thread, THREAD_RECORD_FRAME_TOP_OFFSET);
            b.store_word(base, FRAME_RECORD_PREV_OFFSET, prev);
            b.store_word(base, FRAME_RECORD_FUNCTION_OFFSET, function);
            b.store_word(base, FRAME_RECORD_CODE_OFFSET, code);
            b.store_word(base, FRAME_RECORD_FLAGS_OFFSET, zero);
            b.store_word(base, FRAME_RECORD_OBSERVER_OFFSET, zero);
            let absent = b.constant(FRAME_LOCATION_ABSENT);
            b.store_word(base, FRAME_LOCATION_OFFSET, absent);
            generate_unbound_homes(b, base, homes);
            b.store_word(thread, THREAD_RECORD_PENDING_INVOCATION_OFFSET, zero);
            b.retain(function);
            b.retain(code);
            b.store_word(thread, THREAD_RECORD_FRAME_TOP_OFFSET, base);
            b.set(result, base);
        },
        |b| {
            let base = b.enter_slow(code_slot);
            b.set(result, base);
        },
    );
    result
}

const _: () = assert!(FRAME_HOME_UNBOUND == 0);

fn generate_unbound_homes<B: FrameBuilder>(b: &mut B, base: B::Word, homes: HomeCount<B::Word>) {
    let zero = b.constant(0);
    match homes {
        HomeCount::Constant(count) => {
            for slot in 0..count {
                let offset = (i64::from(slot) * FRAME_HOME_BYTES) as i32;
                b.store_word(base, offset + FRAME_HOME_KIND_OFFSET, zero);
                b.store_word(base, offset + FRAME_HOME_BITS_OFFSET, zero);
            }
        }
        HomeCount::Dynamic(count) => b.counted_loop(count, false, |b, index| {
            let offset = b.shift_left(index, 4);
            let home = b.add(base, offset);
            let zero = b.constant(0);
            b.store_word(home, FRAME_HOME_KIND_OFFSET, zero);
            b.store_word(home, FRAME_HOME_BITS_OFFSET, zero);
        }),
    }
}

fn generate_home_release<B: FrameBuilder>(b: &mut B, home: B::Word, offset: i32) {
    let kind = b.load_word(home, offset + FRAME_HOME_KIND_OFFSET);
    let holds = b.and(kind, FRAME_HOME_HOLDS_REFERENCE as u64);
    let holds = compare_immediate(b, WordComparison::NotEqual, holds, 0);
    b.if_then(holds, |b| {
        let bits = b.load_word(home, offset + FRAME_HOME_BITS_OFFSET);
        b.release(bits);
    });
}

/// Generate a synchronous frame's exit: once on every path that retires the
/// frame [`generate_frame_push`] entered.
///
/// Requires: `base` is that push's result; `homes` and `descending` are the
/// frame's plan.
///
/// Ensures: a zero base pops nothing. A record with a zero flags word is
/// unlinked first, so every finalizer its releases run sees the caller; its
/// homes are released in the target's clear order, then its function and
/// code; the arena top returns to the record's start only after those
/// releases. Any other record exits through the runtime.
pub fn generate_frame_pop<B: FrameBuilder>(
    b: &mut B,
    thread: B::Word,
    base: B::Word,
    homes: HomeCount<B::Word>,
    descending: bool,
) {
    use WordComparison::{Equal, NotEqual};
    let entered = compare_immediate(b, NotEqual, base, 0);
    b.if_then(entered, |b| {
        b.attempt(
            |b| {
                let flags = b.load_word(base, FRAME_RECORD_FLAGS_OFFSET);
                let plain = compare_immediate(b, Equal, flags, 0);
                b.require(plain);
                let prev = b.load_word(base, FRAME_RECORD_PREV_OFFSET);
                b.store_word(thread, THREAD_RECORD_FRAME_TOP_OFFSET, prev);
                match homes {
                    HomeCount::Constant(count) if count <= FRAME_POP_INLINE_HOMES => {
                        let release = |b: &mut B, slot: u32| {
                            let offset = (i64::from(slot) * FRAME_HOME_BYTES) as i32;
                            generate_home_release(b, base, offset);
                        };
                        if descending {
                            (0..count).rev().for_each(|slot| release(b, slot));
                        } else {
                            (0..count).for_each(|slot| release(b, slot));
                        }
                    }
                    HomeCount::Constant(_) | HomeCount::Dynamic(_) => {
                        let count = match homes {
                            HomeCount::Constant(count) => b.constant(u64::from(count)),
                            HomeCount::Dynamic(count) => count,
                        };
                        b.counted_loop(count, descending, |b, index| {
                            let offset = b.shift_left(index, 4);
                            let home = b.add(base, offset);
                            generate_home_release(b, home, 0);
                        });
                    }
                }
                let function = b.load_word(base, FRAME_RECORD_FUNCTION_OFFSET);
                b.release(function);
                let code = b.load_word(base, FRAME_RECORD_CODE_OFFSET);
                b.release(code);
                let start = b.add_immediate(base, -FRAME_RECORD_HEADER_BYTES);
                b.store_word(thread, THREAD_RECORD_ARENA_TOP_OFFSET, start);
            },
            |b| b.exit_slow(base),
        );
    });
}

/// Generate the exit of a frame the runtime linked (a module body, a
/// stateful activation): once on every path that retires it.
///
/// Requires: `base` is the result of the frame's `enter_slow`.
///
/// Ensures: a zero base pops nothing; any other base exits through the
/// runtime, which owns every edge of such a record.
pub fn generate_linked_frame_pop<B: FrameBuilder>(b: &mut B, base: B::Word) {
    let entered = compare_immediate(b, WordComparison::NotEqual, base, 0);
    b.if_then(entered, |b| b.exit_slow(base));
}

#[cfg(test)]
mod tests;
