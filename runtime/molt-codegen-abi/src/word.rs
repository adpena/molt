//! Backend instruction spelling for 64-bit machine words.
//!
//! Every generator in this crate (NaN-box primitives in `boxed_word`,
//! owned-reference transitions in `inline_refcount`) is written once against
//! this trait. An implementation spells instructions only: no encoding
//! constant, eligibility rule or control flow belongs in it.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordComparison {
    Equal,
    NotEqual,
    UnsignedLessThan,
}

pub trait WordBuilder: Sized {
    type Word: Copy;
    type Predicate: Copy;

    fn constant(&mut self, value: u64) -> Self::Word;
    fn and(&mut self, value: Self::Word, mask: u64) -> Self::Word;
    fn or(&mut self, value: Self::Word, bits: u64) -> Self::Word;
    fn shift_left(&mut self, value: Self::Word, amount: u32) -> Self::Word;
    fn shift_right_arithmetic(&mut self, value: Self::Word, amount: u32) -> Self::Word;
    /// Wrapping 64-bit addition of a signed immediate.
    fn add_immediate(&mut self, value: Self::Word, delta: i64) -> Self::Word;
    fn compare(
        &mut self,
        comparison: WordComparison,
        left: Self::Word,
        right: Self::Word,
    ) -> Self::Predicate;
    fn both(&mut self, left: Self::Predicate, right: Self::Predicate) -> Self::Predicate;
}

/// Implements `WordBuilder` for a wrapper by forwarding to one field that
/// already implements it, so a backend spells its word instructions once and
/// every builder that extends `WordBuilder` (for example `RefcountBuilder`)
/// reuses that spelling. Use inside `impl WordBuilder for Wrapper { ... }`:
/// `forward_word_builder!(field: FieldType);`
#[macro_export]
macro_rules! forward_word_builder {
    ($field:ident : $inner:ty) => {
        type Word = <$inner as $crate::word::WordBuilder>::Word;
        type Predicate = <$inner as $crate::word::WordBuilder>::Predicate;
        fn constant(&mut self, value: u64) -> Self::Word {
            $crate::word::WordBuilder::constant(&mut self.$field, value)
        }
        fn and(&mut self, value: Self::Word, mask: u64) -> Self::Word {
            $crate::word::WordBuilder::and(&mut self.$field, value, mask)
        }
        fn or(&mut self, value: Self::Word, bits: u64) -> Self::Word {
            $crate::word::WordBuilder::or(&mut self.$field, value, bits)
        }
        fn shift_left(&mut self, value: Self::Word, amount: u32) -> Self::Word {
            $crate::word::WordBuilder::shift_left(&mut self.$field, value, amount)
        }
        fn shift_right_arithmetic(&mut self, value: Self::Word, amount: u32) -> Self::Word {
            $crate::word::WordBuilder::shift_right_arithmetic(&mut self.$field, value, amount)
        }
        fn add_immediate(&mut self, value: Self::Word, delta: i64) -> Self::Word {
            $crate::word::WordBuilder::add_immediate(&mut self.$field, value, delta)
        }
        fn compare(
            &mut self,
            comparison: $crate::word::WordComparison,
            left: Self::Word,
            right: Self::Word,
        ) -> Self::Predicate {
            $crate::word::WordBuilder::compare(&mut self.$field, comparison, left, right)
        }
        fn both(&mut self, left: Self::Predicate, right: Self::Predicate) -> Self::Predicate {
            $crate::word::WordBuilder::both(&mut self.$field, left, right)
        }
    };
}
