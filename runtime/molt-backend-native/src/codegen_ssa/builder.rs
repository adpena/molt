//! The instruction-selection trait every native backend implements for the
//! walker (design: `docs/design/foundation/codegen_walker.md`, W2).
//!
//! It extends the word and branch builders the `molt-codegen-abi` generators
//! already use, so one backend value spells both. A method names a machine
//! operation, never a TIR op: what an op means is written once in the walker
//! (`scalar` and the families that follow it). The trait grows only by what a
//! migrated family needs.

use molt_codegen_abi::word::BranchBuilder;
use molt_ir::runtime_fn_generated::RuntimeFn;

/// The machine shape of a backend value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MachineKind {
    /// A one-bit truth value.
    Bit,
    /// An integer narrower than a word that is not a truth value (a runtime
    /// entry's 32-bit result).
    NarrowInt,
    /// A 64-bit integer: a raw integer or a boxed word.
    Word,
    /// A 64-bit float.
    Float,
    /// A machine address.
    Address,
}

/// A comparison of two floats. Every ordered comparison is false when either
/// operand is NaN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloatComparison {
    OrderedEqual,
    /// True when the operands differ or either is NaN (`nan != nan`,
    /// `bool(nan)`).
    UnorderedOrNotEqual,
    OrderedLess,
    OrderedLessOrEqual,
    OrderedGreater,
    OrderedGreaterOrEqual,
}

/// A comparison of two words as signed integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntComparison {
    Equal,
    NotEqual,
    SignedLess,
    SignedLessOrEqual,
    SignedGreater,
    SignedGreaterOrEqual,
}

/// A two-operand machine integer instruction. Arithmetic wraps; a shift
/// count outside `[0, 63]` and a zero divisor are undefined, so the walker
/// proves them first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntBinary {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    ShiftLeft,
    ShiftRightArithmetic,
    /// The quotient truncated toward zero.
    SignedQuotient,
    /// The remainder with the dividend's sign.
    SignedRemainder,
}

/// A machine integer instruction that also answers whether the signed
/// result overflowed 64 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckedBinary {
    Add,
    Mul,
}

/// A two-operand IEEE float instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloatBinary {
    Add,
    Sub,
    Mul,
    Divide,
    /// The remainder with the dividend's sign (C `fmod`).
    Remainder,
}

pub(crate) trait CodegenBuilder: BranchBuilder {
    /// Any machine value the backend holds for a TIR value.
    type Value: Copy;
    /// A block of the function being built.
    type Block: Copy;

    fn kind(&self, value: Self::Value) -> MachineKind;
    /// A word as a general value.
    fn word_value(&self, word: Self::Word) -> Self::Value;
    /// A truth bit as a general value.
    fn bit_value(&self, bit: Self::Predicate) -> Self::Value;
    /// The word a `MachineKind::Word` value is.
    fn expect_word(&self, value: Self::Value) -> Self::Word;
    /// The bit a `MachineKind::Bit` value is.
    fn expect_bit(&self, value: Self::Value) -> Self::Predicate;

    /// Zero-extend a `Bit` or `NarrowInt` value to a word.
    fn zero_extend(&mut self, value: Self::Value) -> Self::Word;
    /// The low bit of a word as a truth bit.
    fn low_bit(&mut self, word: Self::Word) -> Self::Predicate;
    /// The 64 bits of a `Float` value, unchanged.
    fn float_to_bits(&mut self, value: Self::Value) -> Self::Word;
    /// The float whose 64 bits are `word`.
    fn bits_to_float(&mut self, word: Self::Word) -> Self::Value;
    /// An `Address` value as a word.
    fn address_to_word(&mut self, value: Self::Value) -> Self::Word;

    /// `no_signed_wrap` is a hint that the walker proved the signed result
    /// exact; a backend with no use for it ignores it.
    fn int_binary(
        &mut self,
        op: IntBinary,
        left: Self::Word,
        right: Self::Word,
        no_signed_wrap: bool,
    ) -> Self::Word;
    fn int_compare(
        &mut self,
        comparison: IntComparison,
        left: Self::Word,
        right: Self::Word,
    ) -> Self::Predicate;
    /// The wrapping result and its signed-overflow bit.
    fn int_checked(
        &mut self,
        op: CheckedBinary,
        left: Self::Word,
        right: Self::Word,
    ) -> (Self::Word, Self::Predicate);
    /// The float nearest a signed integer.
    fn int_to_float(&mut self, word: Self::Word) -> Self::Value;
    /// `yes` when `condition` holds, else `no`; both are of one kind.
    fn select(
        &mut self,
        condition: Self::Predicate,
        yes: Self::Value,
        no: Self::Value,
    ) -> Self::Value;
    fn bit_xor(&mut self, left: Self::Predicate, right: Self::Predicate) -> Self::Predicate;
    /// Two's-complement negation.
    fn int_negate(&mut self, word: Self::Word) -> Self::Word;
    /// Every bit flipped.
    fn int_invert(&mut self, word: Self::Word) -> Self::Word;
    fn bit_not(&mut self, bit: Self::Predicate) -> Self::Predicate;

    /// `fast_math` is a hint that the op carries the fast-math annotation; a
    /// backend with no relaxed float mode ignores it.
    fn float_binary(
        &mut self,
        op: FloatBinary,
        left: Self::Value,
        right: Self::Value,
        fast_math: bool,
    ) -> Self::Value;
    fn float_negate(&mut self, value: Self::Value) -> Self::Value;
    fn float_constant(&mut self, value: f64) -> Self::Value;
    fn float_compare(
        &mut self,
        comparison: FloatComparison,
        left: Self::Value,
        right: Self::Value,
    ) -> Self::Predicate;

    /// Call a runtime entry whose parameters are all words, by its typed
    /// identity; the generated record supplies symbol, signature and context.
    /// The result is `None` when the entry returns nothing.
    fn call_runtime(&mut self, function: RuntimeFn, args: &[Self::Word]) -> Option<Self::Word>;

    /// A value of machine kind `kind` that stands where the lowering has
    /// already refused the function: it keeps the function well formed for
    /// the next diagnostic and never executes.
    fn placeholder(&mut self, kind: MachineKind) -> Self::Value;

    /// Append a parameter of machine kind `kind` to `block`, before the first
    /// branch to it. The result is the parameter's value inside the block;
    /// every branch to `block` passes one argument per parameter, in order.
    /// LLVM builds a phi; Cranelift has block parameters natively.
    fn append_block_param(&mut self, block: Self::Block, kind: MachineKind) -> Self::Value;
    /// End the current block with a jump that passes `args` to `target`.
    fn jump(&mut self, target: Self::Block, args: &[Self::Value]);
    /// End the current block with a two-way branch on `condition`.
    fn branch_if(
        &mut self,
        condition: Self::Predicate,
        yes: (Self::Block, &[Self::Value]),
        no: (Self::Block, &[Self::Value]),
    );
    /// End the current block with a branch on `selector`: the case equal to
    /// it, else `default`.
    fn switch(
        &mut self,
        selector: Self::Word,
        cases: &[(i64, Self::Block, Vec<Self::Value>)],
        default: (Self::Block, &[Self::Value]),
    );
    /// End the current block on a path that never executes.
    fn unreachable(&mut self);
}
