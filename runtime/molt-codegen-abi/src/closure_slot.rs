//! One generator for a word in an object's closure payload: the captured
//! state a generator, coroutine or closure keeps at fixed offsets
//! (`ClosureLoad` / `ClosureStore`). The meaning is molt-runtime
//! `molt_closure_load` / `molt_closure_store` (`builtins/functions/
//! function_abi.rs`, `object/payload_refs.rs`); every backend selects
//! instructions for these generators and keeps the runtime entry only as the
//! store's cold path.
//!
//! ```text
//!   load:  word = owner[offset]; retain(word)               (never cold)
//!   store: a heap value needs the owner's HAS_PTRS flag already set
//!            hit  -> retain(value); owner[offset] = value; release(old)
//!            miss -> molt_closure_store(owner, offset, value)
//! ```
//!
//! The runtime marks HAS_PTRS on the first heap store; it is sticky, so a
//! store into a fresh owner goes cold once and every later store stays
//! inline. Retain-then-release is exact for a self-assignment too.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::frame_record::FrameBuilder;
use crate::list_item::ListLocking;
use crate::word::WordComparison;
use crate::{HEADER_FLAG_HAS_PTRS, HEADER_FLAGS_OFFSET};

/// The owned word at `owner + offset`.
///
/// Requires: `owner` is the raw address of a live object whose closure
/// payload holds a boxed word at `offset`. Ensures: the result is retained
/// for the caller; nothing else changes.
pub fn generate_closure_load<B: FrameBuilder>(
    b: &mut B,
    owner: B::Word,
    offset: B::Word,
) -> B::Word {
    let slot = b.add(owner, offset);
    let value = b.load_word(slot, 0);
    b.retain(value);
    value
}

/// Store the borrowed `value` at `owner + offset`, or call `cold`, the
/// runtime entry that owns every other case.
///
/// Requires: as [`generate_closure_load`]; `value` is borrowed. Ensures: the
/// slot owns one reference to `value` and dropped the one it held.
pub fn generate_closure_store<B: FrameBuilder>(
    b: &mut B,
    owner: B::Word,
    offset: B::Word,
    value: B::Word,
    locking: ListLocking,
    cold: impl FnOnce(&mut B),
) {
    if locking == ListLocking::FreeThreaded {
        cold(b);
        return;
    }
    b.attempt(
        |b| {
            let is_heap = generate_has_tag(b, value, BoxTag::Ptr);
            b.if_then(is_heap, |b| {
                let flags = b.load_word(owner, HEADER_FLAGS_OFFSET);
                let marked = b.and(flags, u64::from(HEADER_FLAG_HAS_PTRS));
                let zero = b.constant(0);
                let has_ptrs = b.compare(WordComparison::NotEqual, marked, zero);
                b.require(has_ptrs);
            });
            let slot = b.add(owner, offset);
            let old = b.load_word(slot, 0);
            b.retain(value);
            b.store_word(slot, 0, value);
            b.release(old);
        },
        cold,
    );
}

#[cfg(test)]
mod tests {
    //! The oracle is the runtime's own rule: a store retains the new value
    //! before releasing the old, and HAS_PTRS is set by the runtime's first
    //! heap store, which the inline path never skips.

    use super::*;
    use crate::test_machine::{Event, Machine};
    use crate::{QNAN, TAG_PTR, box_int_bits};

    const OWNER: u64 = 0x1_0000;
    const OFFSET: u64 = 24;

    fn heap(address: u64) -> u64 {
        QNAN | TAG_PTR | address
    }

    fn owner(has_ptrs: bool, slot: u64) -> Machine {
        let mut m = Machine::default();
        m.write_u32(
            OWNER,
            HEADER_FLAGS_OFFSET,
            if has_ptrs { HEADER_FLAG_HAS_PTRS } else { 0 },
        );
        m.write_u32(OWNER, HEADER_FLAGS_OFFSET + 4, 0);
        m.write_word(OWNER, OFFSET as i32, slot);
        m
    }

    fn store(m: &mut Machine, value: u64) {
        generate_closure_store(m, OWNER, OFFSET, value, ListLocking::Gil, |m| {
            m.events.push(Event::Cold)
        });
    }

    #[test]
    fn a_load_returns_the_slot_retained() {
        let word = heap(0x5_0000);
        let mut m = owner(true, word);
        assert_eq!(generate_closure_load(&mut m, OWNER, OFFSET), word);
        assert_eq!(m.events, [Event::Retain(word)]);
    }

    #[test]
    fn a_store_retains_the_new_word_before_releasing_the_old() {
        let (old, new) = (heap(0x5_0000), heap(0x6_0000));
        let mut m = owner(true, old);
        store(&mut m, new);
        assert_eq!(m.read_bytes(OWNER, OFFSET as i32, 8), new);
        assert_eq!(m.events, [Event::Retain(new), Event::Release(old)]);
    }

    #[test]
    fn a_heap_store_into_an_owner_without_has_ptrs_takes_the_runtime() {
        // The runtime must set the sticky flag; the slot stays untouched.
        let old = box_int_bits(1) as u64;
        let mut m = owner(false, old);
        store(&mut m, heap(0x6_0000));
        assert_eq!(m.events, [Event::Cold]);
        assert_eq!(m.read_bytes(OWNER, OFFSET as i32, 8), old);
    }

    #[test]
    fn an_immediate_store_needs_no_flag() {
        let (old, new) = (box_int_bits(1) as u64, box_int_bits(2) as u64);
        let mut m = owner(false, old);
        store(&mut m, new);
        assert_eq!(m.read_bytes(OWNER, OFFSET as i32, 8), new);
        assert_eq!(m.events, [Event::Retain(new), Event::Release(old)]);
    }

    #[test]
    fn a_self_assignment_keeps_one_reference() {
        let word = heap(0x5_0000);
        let mut m = owner(true, word);
        store(&mut m, word);
        assert_eq!(m.read_bytes(OWNER, OFFSET as i32, 8), word);
        assert_eq!(m.events, [Event::Retain(word), Event::Release(word)]);
    }

    #[test]
    fn a_free_threaded_store_always_takes_the_runtime() {
        let mut m = owner(true, box_int_bits(1) as u64);
        generate_closure_store(
            &mut m,
            OWNER,
            OFFSET,
            heap(0x6_0000),
            ListLocking::FreeThreaded,
            |m| m.events.push(Event::Cold),
        );
        assert_eq!(m.events, [Event::Cold]);
    }
}
