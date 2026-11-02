//! One builder-parametric generator for compiled owned-reference transitions.
//!
//! The runtime's checked algebra remains the transition authority. This
//! generator specializes it to one owner, with explicit control flow and an
//! unchanged slow operand. The terminal release has not been applied when it
//! reaches the runtime: that authority performs the sole 1 -> 0 transaction.

use crate::{
    HEADER_FLAG_DEALLOCATING, HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAG_IMMORTAL, HEADER_FLAGS_OFFSET,
    HEADER_REFCOUNT_OFFSET, HEADER_TYPE_ID_OFFSET, IMMORTAL_REFCOUNT, POINTER_MASK,
    QNAN_TAG_MASK_I64, QNAN_TAG_PTR_I64,
};

/// A snapshot borrowed for exactly one compiled invocation, not a GIL token.
/// The runtime grants plain access only under its full execution attachment,
/// lifecycle lease and GIL, with transition diagnostics disabled. A suspension
/// ends that invocation; resumption obtains a new admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum CompiledRefcountAdmission {
    Runtime = 0,
    AttachedPlain = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefcountOperation {
    Retain,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefcountStorage {
    Plain,
    Atomic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefcountComparison {
    Equal,
    NotEqual,
    UnsignedLessThan,
}

/// Backend instruction spelling only. Control flow and eligibility are owned
/// by `generate_refcount`, never restated by an adapter. `if_else` must finish
/// in its explicit continuation; any caller building a phi captures that
/// continuation after generation, rather than retaining its original block.
pub trait RefcountBuilder: Sized {
    type Word: Copy;
    type Predicate: Copy;

    fn pointer_width(&self) -> crate::PointerWidth;

    fn constant(&mut self, value: u64) -> Self::Word;
    fn and(&mut self, value: Self::Word, mask: u64) -> Self::Word;
    fn add(&mut self, value: Self::Word, delta: i64) -> Self::Word;
    fn shift_right(&mut self, value: Self::Word, amount: Self::Word) -> Self::Word;
    fn compare(
        &mut self,
        comparison: RefcountComparison,
        left: Self::Word,
        right: Self::Word,
    ) -> Self::Predicate;
    fn either(&mut self, left: Self::Predicate, right: Self::Predicate) -> Self::Predicate;
    fn both(&mut self, left: Self::Predicate, right: Self::Predicate) -> Self::Predicate;
    fn load_header_u32(&mut self, pointer: Self::Word, offset: i32) -> Self::Word;
    fn store_header_u32(&mut self, pointer: Self::Word, offset: i32, value: Self::Word);
    fn slow_call(&mut self, operation: RefcountOperation, bits: Self::Word);
    fn if_else(
        &mut self,
        condition: Self::Predicate,
        yes: impl FnOnce(&mut Self),
        no: impl FnOnce(&mut Self),
    );
}

fn compare_immediate<B: RefcountBuilder>(
    b: &mut B,
    comparison: RefcountComparison,
    value: B::Word,
    immediate: u64,
) -> B::Predicate {
    let immediate = b.constant(immediate);
    b.compare(comparison, value, immediate)
}

/// Generate the entire checked fast/slow decision. No header is read before
/// pointer-tag, null and full-execution-admission checks. Atomic builds retain
/// their existing runtime transition, with no invented owner-thread identity.
pub fn generate_refcount<B: RefcountBuilder>(
    b: &mut B,
    bits: B::Word,
    admission: B::Word,
    operation: RefcountOperation,
    storage: RefcountStorage,
) {
    use RefcountComparison::*;
    let tag = b.and(bits, QNAN_TAG_MASK_I64 as u64);
    let pointer_tag = compare_immediate(b, Equal, tag, QNAN_TAG_PTR_I64 as u64);
    b.if_else(
        pointer_tag,
        |b| {
            let pointer = b.and(bits, POINTER_MASK);
            let nonnull = compare_immediate(b, NotEqual, pointer, 0);
            b.if_else(
                nonnull,
                |b| {
                    if storage == RefcountStorage::Atomic {
                        b.slow_call(operation, bits);
                        return;
                    }
                    let admitted = compare_immediate(
                        b,
                        Equal,
                        admission,
                        CompiledRefcountAdmission::AttachedPlain as u64,
                    );
                    let admitted = match b.pointer_width() {
                        crate::PointerWidth::Bits32 => {
                            let fits = compare_immediate(b, UnsignedLessThan, pointer, 1_u64 << 32);
                            b.both(admitted, fits)
                        }
                        crate::PointerWidth::Bits64 => admitted,
                        crate::PointerWidth::Unknown => {
                            panic!("refcount generator requires target pointer width")
                        }
                    };
                    b.if_else(
                        admitted,
                        |b| {
                            generate_attached_transition(b, bits, pointer, operation);
                        },
                        |b| b.slow_call(operation, bits),
                    );
                },
                |_| {},
            );
        },
        |_| {},
    );
}

fn generate_attached_transition<B: RefcountBuilder>(
    b: &mut B,
    bits: B::Word,
    pointer: B::Word,
    operation: RefcountOperation,
) {
    use RefcountComparison::*;
    let type_id = b.load_header_u32(pointer, HEADER_TYPE_ID_OFFSET);
    // This exact sparse membership projection is generated from heap_kinds.toml.
    // A range check alone admits retired ids and is not the runtime boundary.
    let object = compare_immediate(b, Equal, type_id, crate::TYPE_ID_OBJECT as u64);
    let index = b.add(type_id, -(crate::MIN_HEAP_TYPE_ID as i64));
    let in_range = compare_immediate(b, UnsignedLessThan, index, crate::HEAP_TYPE_ID_SPAN);
    let mask = b.constant(crate::HEAP_TYPE_ID_VALID_MASK);
    // LLVM's shift is poison for an out-of-range index even when `in_range`
    // is false. Mask it before evaluation; membership still requires range.
    let shift = b.and(index, 63);
    let present = b.shift_right(mask, shift);
    let present = b.and(present, 1);
    let present = compare_immediate(b, NotEqual, present, 0);
    let dense_member = b.both(in_range, present);
    let valid = b.either(object, dense_member);
    b.if_else(
        valid,
        |b| {
            let flags = b.load_header_u32(pointer, HEADER_FLAGS_OFFSET);
            let immortal = b.and(flags, HEADER_FLAG_IMMORTAL as u64);
            let mortal = compare_immediate(b, Equal, immortal, 0);
            b.if_else(
                mortal,
                |b| {
                    let rejected = HEADER_FLAG_HAS_ABI_VIEW
                        | match operation {
                            RefcountOperation::Retain => HEADER_FLAG_DEALLOCATING,
                            RefcountOperation::Release => 0,
                        };
                    let rejected = b.and(flags, rejected as u64);
                    let ordinary = compare_immediate(b, Equal, rejected, 0);
                    b.if_else(
                        ordinary,
                        |b| {
                            let count = b.load_header_u32(pointer, HEADER_REFCOUNT_OFFSET);
                            // Retain: 1..MAX-2. Release: 2..MAX-1. The terminal edge,
                            // zero, corrupt immortal sentinel and overflow stay untouched
                            // until the checked runtime authority sees the original bits.
                            let minimum = match operation {
                                RefcountOperation::Retain => 1,
                                RefcountOperation::Release => 2,
                            };
                            let index = b.add(count, -minimum);
                            let ordinary_count = compare_immediate(
                                b,
                                UnsignedLessThan,
                                index,
                                (IMMORTAL_REFCOUNT - 2) as u64,
                            );
                            b.if_else(
                                ordinary_count,
                                |b| {
                                    let delta = match operation {
                                        RefcountOperation::Retain => 1,
                                        RefcountOperation::Release => -1,
                                    };
                                    let next = b.add(count, delta);
                                    b.store_header_u32(pointer, HEADER_REFCOUNT_OFFSET, next);
                                },
                                |b| b.slow_call(operation, bits),
                            );
                        },
                        |b| b.slow_call(operation, bits),
                    );
                },
                |_| {},
            );
        },
        |b| b.slow_call(operation, bits),
    );
}

#[cfg(test)]
mod tests;
