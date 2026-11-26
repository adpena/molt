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
//! A stateful activation (a generator, coroutine or async-generator poll)
//! links its task's persistent record on each resume and unlinks it on each
//! suspension, both inline ([`generate_stateful_frame_link`] and
//! [`generate_stateful_frame_unlink`]).
//!
//! A backend spells instructions only. Every layout constant, the handoff
//! test, the release order and the arena discipline live here.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::thread_record::RecordWordBuilder;
use crate::word::{WordBuilder, WordComparison};
use crate::{
    CODE_FRAME_SLOT_OFFSET, FRAME_HOME_BITS_OFFSET, FRAME_HOME_BYTES, FRAME_HOME_HOLDS_REFERENCE,
    FRAME_HOME_KIND_OFFSET, FRAME_HOME_UNBOUND, FRAME_LOCATION_OFFSET, FRAME_RECORD_CODE_OFFSET,
    FRAME_RECORD_FLAGS_OFFSET, FRAME_RECORD_FUNCTION_OFFSET, FRAME_RECORD_HEADER_BYTES,
    FRAME_RECORD_OBSERVER_OFFSET, FRAME_RECORD_PREV_OFFSET, FRAME_RECORD_STATEFUL,
    FUNCTION_CODE_OFFSET, HEADER_AUX_OFFSET, HEADER_TYPE_ID_OFFSET, POINTER_MASK,
    SIDECAR_FRAME_RECORD_BASE_OFFSET, THREAD_RECORD_ARENA_LIMIT_OFFSET,
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
/// The `trace_enter_slot` value of a stateful activation's entry (a
/// generator, coroutine or async-generator poll): it links its task's
/// persistent record, whose bindings live in the task payload.
pub const FRAME_ENTRY_STATEFUL: i64 = 1 << 33;

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

    /// Bytes the physical entry would occupy in the current arena chunk.
    pub const fn arena_bytes(self) -> u64 {
        FRAME_RECORD_HEADER_BYTES as u64 + self.homes as u64 * FRAME_HOME_BYTES as u64
    }
}

/// Occupancy of the composed activation before a particular logical entry.
/// Physical parents are already reflected in the thread record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VirtualFramePrefix {
    pub arena_bytes: u32,
    pub recursion_depth: u32,
}

impl VirtualFramePrefix {
    pub fn enter(self, shape: FrameShape) -> Option<Self> {
        Some(Self {
            arena_bytes: self
                .arena_bytes
                .checked_add(u32::try_from(shape.arena_bytes()).ok()?)?,
            recursion_depth: self.recursion_depth.checked_add(1)?,
        })
    }
}

/// How a compiled frame entry links its record: the decoded
/// `trace_enter_slot` value. An entry with no value is runtime-linked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameEntryKind {
    /// A synchronous function pushes and pops an arena record inline.
    Synchronous(FrameShape),
    /// A stateful activation links and unlinks its task's persistent record.
    Stateful,
}

impl FrameEntryKind {
    pub const fn from_entry_value(value: i64) -> Self {
        if value & FRAME_ENTRY_STATEFUL != 0 {
            Self::Stateful
        } else {
            Self::Synchronous(FrameShape::from_entry_value(value))
        }
    }

    /// The synchronous shape, or `None` for a stateful entry.
    pub const fn shape(self) -> Option<FrameShape> {
        match self {
            Self::Synchronous(shape) => Some(shape),
            Self::Stateful => None,
        }
    }
}

/// Backend instruction spelling for frame transitions, extending the
/// backend's one `RecordWordBuilder`. Control flow and layout are owned by
/// the generators below.
pub trait FrameBuilder: RecordWordBuilder {
    /// A mutable word that survives control flow.
    type Local: Copy;

    fn local(&mut self) -> Self::Local;
    fn set(&mut self, local: Self::Local, value: Self::Word);
    fn get(&mut self, local: Self::Local) -> Self::Word;

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

/// The initialized code and reservation bounds for a frame that can enter
/// without invoking the runtime. Call within an attempt's guarded arm.
fn checked_frame_entry<B: FrameBuilder>(
    b: &mut B,
    thread: B::Word,
    function: B::Word,
    code_slot: B::Word,
    homes: HomeCount<B::Word>,
    occupied: u32,
) -> (B::Word, B::Word, B::Word) {
    use WordComparison::{Equal, UnsignedLessThan};
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
    let base = b.add_immediate(start, FRAME_RECORD_HEADER_BYTES + i64::from(occupied));
    let end = match home_bytes(b, homes) {
        HomeCount::Constant(count) => b.add_immediate(base, i64::from(count) * FRAME_HOME_BYTES),
        HomeCount::Dynamic(bytes) => b.add(base, bytes),
    };
    let limit = b.load_word(thread, THREAD_RECORD_ARENA_LIMIT_OFFSET);
    // end <= limit, as one unsigned compare. An empty arena (top and
    // limit both zero) never fits.
    let past_limit = b.add_immediate(limit, 1);
    let fits = b.compare(UnsignedLessThan, end, past_limit);
    b.require(fits);
    (code, base, end)
}

/// Whether the captured function can enter the current arena through the
/// proved fast path. This does not take the handoff or reserve a record.
pub fn generate_frame_entry_admits<B: FrameBuilder>(
    b: &mut B,
    thread: B::Word,
    function: B::Word,
    code_slot: B::Word,
    homes: u32,
    prefix: VirtualFramePrefix,
) -> B::Word {
    let result = b.local();
    let no = b.constant(0);
    b.set(result, no);
    b.attempt(
        |b| {
            let pending = b.load_word(thread, crate::THREAD_RECORD_EXCEPTION_PENDING_OFFSET);
            let pending = b.and(pending, 0xff);
            let clear = compare_immediate(b, WordComparison::Equal, pending, 0);
            b.require(clear);
            let recursion = crate::thread_record::generate_recursion_admits_after(
                b,
                thread,
                prefix.recursion_depth,
            );
            b.require(recursion);
            checked_frame_entry(
                b,
                thread,
                function,
                code_slot,
                HomeCount::Constant(homes),
                prefix.arena_bytes,
            );
            let yes = b.constant(1);
            b.set(result, yes);
        },
        |_| {},
    );
    b.get(result)
}

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
    let result = b.local();
    b.attempt(
        |b| {
            // The handoff: a function object whose code owns this slot.
            let function = b.load_word(thread, THREAD_RECORD_PENDING_INVOCATION_OFFSET);
            let (code, base, end) = checked_frame_entry(b, thread, function, code_slot, homes, 0);
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

/// Generate a stateful activation's entry: a poll's resume links its task's
/// persistent record, as CPython's `_SEND_GEN_FRAME` links the generator's
/// embedded frame (`frame->previous = tstate->current_frame`). The record's
/// constant facts (code, the activation, its flags) were written once when
/// the task was constructed, and its location word persists across
/// suspensions, so a resume writes two words.
///
/// Requires: `thread` is this invocation's thread record; `payload` is the
/// poll's task payload address, a task `molt_task_new` built (its header's aux
/// word is its sidecar); `code_slot` is the poll's code slot. Runs once per
/// resume, before any frame operation.
///
/// Ensures: the result local holds the base of the record linked as the
/// thread's executing frame. A task whose construction initialized its record
/// (flags exactly `FRAME_RECORD_STATEFUL`) links it inline; any other entry
/// is linked by the runtime, whose zero result means nothing was linked and an
/// exception is pending.
pub fn generate_stateful_frame_link<B: FrameBuilder>(
    b: &mut B,
    thread: B::Word,
    payload: B::Word,
    code_slot: B::Word,
) -> B::Local {
    let result = b.local();
    b.attempt(
        |b| {
            let sidecar = b.load_word(payload, HEADER_AUX_OFFSET);
            let base = b.add_immediate(sidecar, i64::from(SIDECAR_FRAME_RECORD_BASE_OFFSET));
            let flags = b.load_word(base, FRAME_RECORD_FLAGS_OFFSET);
            let initialized =
                compare_immediate(b, WordComparison::Equal, flags, FRAME_RECORD_STATEFUL);
            b.require(initialized);
            let prev = b.load_word(thread, THREAD_RECORD_FRAME_TOP_OFFSET);
            b.store_word(base, FRAME_RECORD_PREV_OFFSET, prev);
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

/// Generate a stateful activation's exit: once on every path that suspends
/// or finishes the frame [`generate_stateful_frame_link`] entered.
///
/// Requires: `base` is that link's result.
///
/// Ensures: a zero base unlinks nothing. A persistent record is unlinked
/// inline and keeps everything it holds for the next resume; a record the
/// runtime linked exits through the runtime.
pub fn generate_stateful_frame_unlink<B: FrameBuilder>(b: &mut B, thread: B::Word, base: B::Word) {
    let entered = compare_immediate(b, WordComparison::NotEqual, base, 0);
    b.if_then(entered, |b| {
        b.attempt(
            |b| {
                let flags = b.load_word(base, FRAME_RECORD_FLAGS_OFFSET);
                let persistent =
                    compare_immediate(b, WordComparison::Equal, flags, FRAME_RECORD_STATEFUL);
                b.require(persistent);
                let prev = b.load_word(base, FRAME_RECORD_PREV_OFFSET);
                b.store_word(thread, THREAD_RECORD_FRAME_TOP_OFFSET, prev);
                let zero = b.constant(0);
                b.store_word(base, FRAME_RECORD_PREV_OFFSET, zero);
            },
            |b| b.exit_slow(base),
        );
    });
}

/// Generate the exit of a frame the runtime linked (a module body, a
/// runtime-linked entry): once on every path that retires it.
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
