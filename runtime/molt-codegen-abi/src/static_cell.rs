//! One builder-parametric generator for a store into a static module cell
//! (call_anatomy.md 4.5a). A cell-eligible module keeps each global of its
//! layout in one word of a fixed-address block in program data; the meaning
//! of a store lives here once, and a backend only spells the primitives
//! (`CellBuilder`). A read is the slot read (`global_slot`) over the block's
//! base: load, unbound test, retain, cold by-name path.
//!
//! ```text
//!   base = StaticCellBase(module)      ; the block, as an inline int of
//!                                      ; (address >> 3): a link-time constant
//!   retain value
//!   old = swap [unbox(base) << 3 + 8k], value
//!   release old                        ; an unbound word is not a pointer:
//!                                      ; its release is the tag test alone
//! ```
//!
//! The cell owns one reference to its value. The new value is retained before
//! the swap and the old one released after it, so a destructor the release
//! runs observes the cell already holding the new value, as CPython's
//! `STORE_GLOBAL` (insert, then decref the old value) does. A free-threaded
//! build swaps atomically; a racing reader keeps the try-retain debt the slot
//! tables document (`molt-runtime/src/builtins/namespace_slots.rs`).

use crate::boxed_word::generate_unbox_inline_int;
use crate::global_slot::{GLOBAL_SLOT_BASE_SHIFT, SlotBuilder};

/// Backend spelling for a cell store, beyond the slot read's primitives.
pub trait CellBuilder: SlotBuilder {
    /// Store `value` into the 64-bit word at `address + offset` and return the
    /// word it held: an atomic exchange when `atomic`, else a load and a store.
    fn swap_word(
        &mut self,
        address: Self::Word,
        offset: i32,
        value: Self::Word,
        atomic: bool,
    ) -> Self::Word;
    /// Release one owned reference to the value `word`.
    fn release(&mut self, word: Self::Word);
}

/// The byte offset of cell `index` in its block.
pub fn static_cell_offset(index: u32) -> i32 {
    i32::try_from(u64::from(index) * 8).expect("static cell offset fits i32")
}

/// Store the owned-by-caller `value` into cell `index` of the block whose
/// boxed base is `base`: the cell takes its own reference and drops the one
/// it held. `atomic` is the build's refcount storage
/// (`MOLT_REFCOUNT_ATOMIC`): a free-threaded runtime swaps atomically.
pub fn generate_static_cell_store<B: CellBuilder>(
    b: &mut B,
    base: B::Word,
    index: u32,
    value: B::Word,
    atomic: bool,
) {
    let shifted = generate_unbox_inline_int(b, base);
    let address = b.shift_left(shifted, GLOBAL_SLOT_BASE_SHIFT);
    b.retain(value);
    let old = b.swap_word(address, static_cell_offset(index), value, atomic);
    b.release(old);
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::NAMESPACE_SLOT_UNBOUND;
    use crate::global_slot::generate_global_slot_read;
    use crate::word::{WordBuilder, WordComparison};
    use std::vec;
    use std::vec::Vec;

    const BASE: u64 = 0x2_0000_0000;

    /// A word interpreter over a fake block. The independent oracle is a
    /// plain array of cells and a reference count per value: every bound cell
    /// owns exactly one reference, and an unbound word owns none.
    #[derive(Default)]
    struct Interp {
        cells: Vec<u64>,
        events: Vec<(&'static str, u64)>,
        atomic_swaps: u32,
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

    impl crate::word::BranchBuilder for Interp {
        fn choose(
            &mut self,
            condition: bool,
            yes: impl FnOnce(&mut Self) -> u64,
            no: impl FnOnce(&mut Self) -> u64,
        ) -> u64 {
            if condition { yes(self) } else { no(self) }
        }
    }

    impl Interp {
        fn cell(&self, address: u64, offset: i32) -> usize {
            let at = address.wrapping_add(offset as i64 as u64) - BASE;
            assert_eq!(at % 8, 0);
            (at / 8) as usize
        }
    }

    impl SlotBuilder for Interp {
        fn load_word(&mut self, address: u64, offset: i32) -> u64 {
            self.cells[self.cell(address, offset)]
        }
        fn retain(&mut self, word: u64) {
            self.events.push(("retain", word));
        }
    }

    impl CellBuilder for Interp {
        fn swap_word(&mut self, address: u64, offset: i32, value: u64, atomic: bool) -> u64 {
            self.atomic_swaps += u32::from(atomic);
            let cell = self.cell(address, offset);
            self.events.push(("swap", value));
            core::mem::replace(&mut self.cells[cell], value)
        }
        fn release(&mut self, word: u64) {
            self.events.push(("release", word));
        }
    }

    fn base() -> u64 {
        crate::box_int_bits((BASE >> GLOBAL_SLOT_BASE_SHIFT) as i64) as u64
    }

    #[test]
    fn a_store_retains_the_new_value_before_it_releases_the_old_one() {
        let (first, second) = (0x7ffc_0000_0000_1000_u64, 0x7ffc_0000_0000_2000_u64);
        let mut b = Interp {
            cells: vec![NAMESPACE_SLOT_UNBOUND; 3],
            ..Default::default()
        };
        generate_static_cell_store(&mut b, base(), 2, first, false);
        generate_static_cell_store(&mut b, base(), 2, second, false);
        assert_eq!(
            b.cells,
            vec![NAMESPACE_SLOT_UNBOUND, NAMESPACE_SLOT_UNBOUND, second]
        );
        assert_eq!(
            b.events,
            vec![
                ("retain", first),
                ("swap", first),
                // The first store finds the cell unbound; releasing that word
                // is the refcount generator's tag test, a no-op.
                ("release", NAMESPACE_SLOT_UNBOUND),
                ("retain", second),
                ("swap", second),
                ("release", first),
            ]
        );
        assert_eq!(b.atomic_swaps, 0);
        generate_static_cell_store(&mut b, base(), 0, first, true);
        assert_eq!(b.atomic_swaps, 1);
    }

    #[test]
    fn a_read_after_a_store_sees_the_stored_value_and_unbound_takes_the_cold_path() {
        let value = crate::box_int_bits(42) as u64;
        let mut b = Interp {
            cells: vec![NAMESPACE_SLOT_UNBOUND; 2],
            ..Default::default()
        };
        let cold = generate_global_slot_read(&mut b, base(), 1, |_| 7);
        assert_eq!(cold, 7);
        generate_static_cell_store(&mut b, base(), 1, value, false);
        let read = generate_global_slot_read(&mut b, base(), 1, |_| 7);
        assert_eq!(read, value);
        assert_eq!(b.events.last(), Some(&("retain", value)));
    }
}
