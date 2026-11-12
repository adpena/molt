//! One builder-parametric generator for a compiled read of a module global
//! slot (call_anatomy.md 4.5). The meaning of the read lives here once;
//! a backend only spells the primitives (`SlotBuilder`).
//!
//! ```text
//!   base = GlobalSlotBase(module)       ; the activation's slot words, as an
//!                                       ; inline int of (address >> 3)
//!   v = load64 [unbox(base) << 3 + 8k]
//!   if v != NAMESPACE_SLOT_UNBOUND { retain v; result = v }
//!   else { result = cold() }            ; the by-name read, unchanged
//! ```
//!
//! A foreign-globals activation's base addresses an all-unbound array, so
//! every read takes the cold path. An unbound word is never retained; a bound
//! one is a value the namespace dictionary owns, retained before use.

use crate::NAMESPACE_SLOT_UNBOUND;
use crate::boxed_word::generate_unbox_inline_int;
use crate::word::{WordBuilder, WordComparison};

/// The slot words' address is published as an inline int of `address >>
/// GLOBAL_SLOT_BASE_SHIFT`: slot words are 8-byte aligned, and the shifted
/// address fits the inline payload on every target.
pub const GLOBAL_SLOT_BASE_SHIFT: u32 = 3;

/// Backend spelling for a slot read: one raw 64-bit load, a two-armed
/// choice that yields one word, and the backend's own retain of a value
/// word (its instantiation of the shared refcount generator). Everything
/// else comes from `WordBuilder`.
pub trait SlotBuilder: WordBuilder {
    /// The 64-bit word at `address + offset`; `address` is a raw address.
    fn load_word(&mut self, address: Self::Word, offset: i32) -> Self::Word;
    /// Run `yes` when `condition` holds, else `no`; the result is the word
    /// the taken arm returned, in the arms' common continuation.
    fn choose(
        &mut self,
        condition: Self::Predicate,
        yes: impl FnOnce(&mut Self) -> Self::Word,
        no: impl FnOnce(&mut Self) -> Self::Word,
    ) -> Self::Word;
    /// Take one owned reference to the value `word`.
    fn retain(&mut self, word: Self::Word);
}

/// The owned value of slot `index`, or what `cold` returns when the slot is
/// unbound. `base` is the boxed base word of the activation.
pub fn generate_global_slot_read<B: SlotBuilder>(
    b: &mut B,
    base: B::Word,
    index: u32,
    cold: impl FnOnce(&mut B) -> B::Word,
) -> B::Word {
    let shifted = generate_unbox_inline_int(b, base);
    let address = b.shift_left(shifted, GLOBAL_SLOT_BASE_SHIFT);
    let offset = i32::try_from(u64::from(index) * 8).expect("global slot offset fits i32");
    let value = b.load_word(address, offset);
    let unbound = b.constant(NAMESPACE_SLOT_UNBOUND);
    let bound = b.compare(WordComparison::NotEqual, value, unbound);
    b.choose(
        bound,
        |b| {
            b.retain(value);
            value
        },
        cold,
    )
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn box_int(value: i64) -> u64 {
        crate::box_int_bits(value) as u64
    }

    /// A word interpreter over a fake memory: the independent oracle is the
    /// plain read `memory[(address - BASE) / 8]` and the unbound rule.
    struct Interp {
        memory: Vec<u64>,
        retained: Vec<u64>,
        cold_calls: u32,
    }
    const BASE: u64 = 0x1_0000_0000;

    impl WordBuilder for Interp {
        type Word = u64;
        type Predicate = bool;
        fn constant(&mut self, value: u64) -> u64 {
            value
        }
        fn and(&mut self, value: u64, mask: u64) -> u64 {
            value & mask
        }
        fn or(&mut self, value: u64, bits: u64) -> u64 {
            value | bits
        }
        fn shift_left(&mut self, value: u64, amount: u32) -> u64 {
            value << amount
        }
        fn shift_right_arithmetic(&mut self, value: u64, amount: u32) -> u64 {
            ((value as i64) >> amount) as u64
        }
        fn add_immediate(&mut self, value: u64, delta: i64) -> u64 {
            value.wrapping_add(delta as u64)
        }
        fn compare(&mut self, comparison: WordComparison, left: u64, right: u64) -> bool {
            match comparison {
                WordComparison::Equal => left == right,
                WordComparison::NotEqual => left != right,
                WordComparison::UnsignedLessThan => left < right,
            }
        }
        fn both(&mut self, left: bool, right: bool) -> bool {
            left && right
        }
    }

    impl SlotBuilder for Interp {
        fn load_word(&mut self, address: u64, offset: i32) -> u64 {
            let at = address.wrapping_add(offset as i64 as u64) - BASE;
            assert_eq!(at % 8, 0);
            self.memory[(at / 8) as usize]
        }
        fn choose(
            &mut self,
            condition: bool,
            yes: impl FnOnce(&mut Self) -> u64,
            no: impl FnOnce(&mut Self) -> u64,
        ) -> u64 {
            if condition { yes(self) } else { no(self) }
        }
        fn retain(&mut self, word: u64) {
            self.retained.push(word);
        }
    }

    #[test]
    fn a_bound_slot_is_retained_and_an_unbound_slot_takes_the_cold_path() {
        let value = 0x7ffc_0000_dead_beef_u64;
        let mut b = Interp {
            memory: vec![value, NAMESPACE_SLOT_UNBOUND, box_int(5)],
            retained: Vec::new(),
            cold_calls: 0,
        };
        let base = box_int((BASE >> GLOBAL_SLOT_BASE_SHIFT) as i64);
        for (index, expected) in [(0, value), (2, box_int(5))] {
            let read = generate_global_slot_read(&mut b, base, index, |b| {
                b.cold_calls += 1;
                0
            });
            assert_eq!(read, expected);
        }
        assert_eq!(b.retained, vec![value, box_int(5)]);
        let read = generate_global_slot_read(&mut b, base, 1, |b| {
            b.cold_calls += 1;
            42
        });
        assert_eq!((read, b.cold_calls, b.retained.len()), (42, 1, 2));
    }
}
