//! One builder-parametric generator for a closed import (bedrock §13 D2, M1).
//! A registry row with no writer imports through its ready word: one 8-byte
//! word per row in the program's `molt_module_ready` block. The runtime sets
//! the word to the row's module (a borrowed, NaN-boxed object word) while the
//! row is done and its `sys.modules` entry is that object, and every writer
//! of the entry clears it first (D2a, IC2). The meaning of the import lives
//! here once; a backend only spells the primitives (`ReadyBuilder`).
//!
//! ```text
//!   address = block + (unbox(id) << 3)
//!   word = load.acquire [address]
//!   if word != 0: retain word           ; warm: no runtime call
//!   else:         word = cold(id)       ; molt_module_ensure(id): owns
//!                                       ; recursion, threads, failure,
//!                                       ; retirement; returns owned
//! ```
//!
//! The acquire load pairs with the runtime's release store of the word, so a
//! thread that sees the pointer sees the module's finished body. Zero is never
//! a module word (it is the float 0.0 bit pattern), so it marks "not ready".
//! The result is owned either way: the warm arm takes its own reference and
//! the cold call returns one.

use crate::boxed_word::generate_unbox_inline_int;
use crate::global_slot::SlotBuilder;
use crate::word::WordComparison;

/// Bytes per ready word: row `id`'s word sits at `block + 8 * id`.
pub const MODULE_READY_WORD_SHIFT: u32 = 3;

/// Backend spelling for a closed import, beyond the slot read's primitives.
pub trait ReadyBuilder: SlotBuilder {
    /// Wrapping 64-bit addition of two words.
    fn add(&mut self, left: Self::Word, right: Self::Word) -> Self::Word;
    /// A 64-bit load with acquire ordering at `address + offset`.
    fn load_acquire(&mut self, address: Self::Word, offset: i32) -> Self::Word;
}

/// The owned module of the row whose boxed `ModuleId` is `id`. `block` is
/// the raw address of the program's ready block. `cold` receives the boxed id
/// and returns an owned module (the `molt_module_ensure` call).
pub fn generate_module_import_closed<B: ReadyBuilder>(
    b: &mut B,
    block: B::Word,
    id: B::Word,
    cold: impl FnOnce(&mut B, B::Word) -> B::Word,
) -> B::Word {
    let index = generate_unbox_inline_int(b, id);
    let offset = b.shift_left(index, MODULE_READY_WORD_SHIFT);
    let address = b.add(block, offset);
    let word = b.load_acquire(address, 0);
    let zero = b.constant(0);
    let ready = b.compare(WordComparison::NotEqual, word, zero);
    b.choose(
        ready,
        |b| {
            b.retain(word);
            word
        },
        |b| cold(b, id),
    )
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::word::{BranchBuilder, WordBuilder};
    use std::vec;
    use std::vec::Vec;

    const BLOCK: u64 = 0x3_0000_0000;

    /// A word interpreter over a fake ready block. The independent oracle is
    /// the runtime's contract: a nonzero word is a module the caller now owns
    /// one more reference to; a zero word means the cold ensure runs.
    #[derive(Default)]
    struct Interp {
        words: Vec<u64>,
        events: Vec<(&'static str, u64)>,
        acquire_loads: u32,
    }

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

    impl BranchBuilder for Interp {
        fn choose(
            &mut self,
            condition: bool,
            yes: impl FnOnce(&mut Self) -> u64,
            no: impl FnOnce(&mut Self) -> u64,
        ) -> u64 {
            if condition { yes(self) } else { no(self) }
        }
    }

    impl SlotBuilder for Interp {
        fn load_word(&mut self, _address: u64, _offset: i32) -> u64 {
            panic!("a closed import reads its word with acquire ordering")
        }
        fn retain(&mut self, word: u64) {
            self.events.push(("retain", word));
        }
    }

    impl ReadyBuilder for Interp {
        fn add(&mut self, left: u64, right: u64) -> u64 {
            left.wrapping_add(right)
        }
        fn load_acquire(&mut self, address: u64, offset: i32) -> u64 {
            self.acquire_loads += 1;
            let at = address.wrapping_add(offset as i64 as u64) - BLOCK;
            assert_eq!(at % 8, 0, "ready words are 8-aligned");
            self.words[(at / 8) as usize]
        }
    }

    fn boxed(id: i64) -> u64 {
        crate::box_int_bits(id) as u64
    }

    #[test]
    fn a_ready_word_is_retained_and_returned_without_the_cold_call() {
        let module = 0x7ffc_0000_0000_4000_u64;
        let mut b = Interp {
            words: vec![0, 0, module, 0],
            ..Default::default()
        };
        let got = generate_module_import_closed(&mut b, BLOCK, boxed(2), |_, _| {
            panic!("a ready row must not call ensure")
        });
        assert_eq!(got, module);
        assert_eq!(b.events, vec![("retain", module)]);
        assert_eq!(b.acquire_loads, 1);
    }

    #[test]
    fn a_zero_word_calls_ensure_with_the_boxed_id_and_takes_no_reference() {
        let ensured = 0x7ffc_0000_0000_5000_u64;
        let mut b = Interp {
            words: vec![0x7ffc_0000_0000_6000, 0, 0],
            ..Default::default()
        };
        let mut cold_id = None;
        let got = generate_module_import_closed(&mut b, BLOCK, boxed(1), |_, id| {
            cold_id = Some(id);
            ensured
        });
        assert_eq!(got, ensured);
        assert_eq!(cold_id, Some(boxed(1)), "ensure decodes the boxed id");
        assert!(b.events.is_empty(), "ensure returns an owned module");
    }

    #[test]
    fn each_row_reads_its_own_word() {
        let words: Vec<u64> = (0..5).map(|i| 0x7ffc_0000_0001_0000 + i * 8).collect();
        for (id, &word) in words.iter().enumerate() {
            let mut b = Interp {
                words: words.clone(),
                ..Default::default()
            };
            let got = generate_module_import_closed(&mut b, BLOCK, boxed(id as i64), |_, _| 0);
            assert_eq!(got, word);
        }
    }
}
