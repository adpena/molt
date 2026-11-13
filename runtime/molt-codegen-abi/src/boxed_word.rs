//! NaN-box word primitives, written once for every backend (HF-821).
//!
//! These are the bit-level meaning of the representation ops that the late
//! lowering stage of TIR emits, and of the boxing helpers backends call today.
//! They are primitives only: which branch to take and which runtime entry to
//! call stay in TIR, where the optimizer sees them. Each generator computes
//! exactly what its scalar authority in this crate computes (`tag_bits`,
//! `unbox_inline_int_bits`, `fits_inline_int`, `box_int_bits`); the tests
//! check that equality on every edge and on a word sweep.

use crate::word::{WordBuilder, WordComparison};
use crate::{INLINE_INT_BIAS, INLINE_INT_LIMIT, INT_MASK, INT_SHIFT, QNAN, TAG_MASK};

/// The tag field of a boxed word that is not a float.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoxTag {
    Int,
    Bool,
    None,
    Ptr,
    Pending,
}

impl BoxTag {
    pub const fn bits(self) -> u64 {
        match self {
            Self::Int => crate::TAG_INT,
            Self::Bool => crate::TAG_BOOL,
            Self::None => crate::TAG_NONE,
            Self::Ptr => crate::TAG_PTR,
            Self::Pending => crate::TAG_PENDING,
        }
    }
}

/// True iff `word` carries `tag`: the runtime's `tag_bits(word) == QNAN | tag`.
pub fn generate_has_tag<B: WordBuilder>(b: &mut B, word: B::Word, tag: BoxTag) -> B::Predicate {
    let tag_field = b.and(word, QNAN | TAG_MASK);
    let expected = b.constant(QNAN | tag.bits());
    b.compare(WordComparison::Equal, tag_field, expected)
}

/// The sign-extended 47-bit payload of an inline-int word. Total: on a word
/// without the int tag the result is meaningless but defined, so it may be
/// speculated above the tag test.
pub fn generate_unbox_inline_int<B: WordBuilder>(b: &mut B, word: B::Word) -> B::Word {
    let shift = INT_SHIFT as u32;
    let shifted = b.shift_left(word, shift);
    b.shift_right_arithmetic(shifted, shift)
}

/// True iff the signed `value` fits the inline payload, [-2^46, 2^46): one
/// unsigned compare of the biased value.
pub fn generate_inline_int_fits<B: WordBuilder>(b: &mut B, value: B::Word) -> B::Predicate {
    let biased = b.add_immediate(value, INLINE_INT_BIAS);
    let limit = b.constant(INLINE_INT_LIMIT as u64);
    b.compare(WordComparison::UnsignedLessThan, biased, limit)
}

/// The truth value of a bool word: its low bit. Total: meaningful where
/// `generate_has_tag(.., BoxTag::Bool)` holds.
pub fn generate_unbox_bool<B: WordBuilder>(b: &mut B, word: B::Word) -> B::Predicate {
    let bit = b.and(word, 1);
    let zero = b.constant(0);
    b.compare(WordComparison::NotEqual, bit, zero)
}

/// True iff `word` is a float (`is_float_bits`, runtime spec 2.1.1): its
/// exponent and tag field are below the first tag. A float is its own IEEE
/// bits, read as an f64 by the backend's bit cast, and boxed by identity.
pub fn generate_is_float<B: WordBuilder>(b: &mut B, word: B::Word) -> B::Predicate {
    let shifted = b.shift_left(word, 1);
    let first_tag = b.constant(crate::SHIFTED_FIRST_TAG_BOUNDARY);
    b.compare(WordComparison::UnsignedLessThan, shifted, first_tag)
}

/// The inline-int word for `value`. Requires: `value` fits (a dominating
/// `generate_inline_int_fits` true edge); otherwise the payload wraps.
pub fn generate_box_inline_int<B: WordBuilder>(b: &mut B, value: B::Word) -> B::Word {
    let payload = b.and(value, INT_MASK);
    b.or(payload, QNAN | BoxTag::Int.bits())
}

#[cfg(test)]
mod tests;
