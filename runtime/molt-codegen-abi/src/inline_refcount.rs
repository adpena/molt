//! One builder-parametric generator for compiled owned-reference transitions.
//!
//! The runtime's checked algebra remains the transition authority. This
//! generator specializes it to one owner, with explicit control flow and an
//! unchanged slow operand. The terminal release has not been applied when it
//! reaches the runtime: that authority performs the sole 1 -> 0 transaction.
//!
//! Plain eligibility lives in the count word (`REFCOUNT_RUNTIME_CUSTODY`), so a
//! site is tag/null, admission, one load, one window compare, add and store.

use crate::boxed_word::{BoxTag, generate_has_tag};
use crate::word::{WordBuilder, WordComparison};
use crate::{
    COMPILED_PLAIN_REFCOUNT_WINDOW, HEADER_REFCOUNT_OFFSET, IMMORTAL_REFCOUNT, POINTER_MASK,
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
    /// Debug compilers retain runtime header-kind validation at every site.
    HardenedPlain,
}

impl RefcountStorage {
    pub const fn for_build(atomic: bool) -> Self {
        if atomic {
            Self::Atomic
        } else if cfg!(debug_assertions) {
            Self::HardenedPlain
        } else {
            Self::Plain
        }
    }
}

/// Backend instruction spelling only, extending the backend's one
/// `WordBuilder`. Control flow and eligibility are owned by
/// `generate_refcount`, never restated by an adapter. `if_else` must finish
/// in its explicit continuation; any caller building a phi captures that
/// continuation after generation, rather than retaining its original block.
pub trait RefcountBuilder: WordBuilder {
    fn pointer_width(&self) -> crate::PointerWidth;

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

fn compare_immediate<B: WordBuilder>(
    b: &mut B,
    comparison: WordComparison,
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
    use WordComparison::*;
    let pointer_tag = generate_has_tag(b, bits, BoxTag::Ptr);
    b.if_else(
        pointer_tag,
        |b| {
            let pointer = b.and(bits, POINTER_MASK);
            let nonnull = compare_immediate(b, NotEqual, pointer, 0);
            b.if_else(
                nonnull,
                |b| {
                    if storage != RefcountStorage::Plain {
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

/// The plain transition reads exactly one header word. Eligibility was decided
/// when the object was allocated or changed state: `MoltHeader`'s flag
/// authority sets `REFCOUNT_RUNTIME_CUSTODY` before it publishes any custody
/// flag (immortal, C view, deallocating), and the immortal sentinel carries the
/// bit by construction. One unsigned window compare therefore rejects custody,
/// the terminal edge, zero and overflow; only the immortal no-op is decided
/// inline on the cold side, matching the runtime's own early return.
fn generate_attached_transition<B: RefcountBuilder>(
    b: &mut B,
    bits: B::Word,
    pointer: B::Word,
    operation: RefcountOperation,
) {
    use WordComparison::*;
    let count = b.load_header_u32(pointer, HEADER_REFCOUNT_OFFSET);
    // Retain: 1..=WINDOW. Release: 2..=WINDOW + 1. The runtime authority sees
    // the original bits for every other word.
    let minimum = match operation {
        RefcountOperation::Retain => 1,
        RefcountOperation::Release => 2,
    };
    let index = b.add_immediate(count, -minimum);
    let plain = compare_immediate(b, UnsignedLessThan, index, COMPILED_PLAIN_REFCOUNT_WINDOW);
    b.if_else(
        plain,
        |b| {
            let delta = match operation {
                RefcountOperation::Retain => 1,
                RefcountOperation::Release => -1,
            };
            let next = b.add_immediate(count, delta);
            b.store_header_u32(pointer, HEADER_REFCOUNT_OFFSET, next);
        },
        |b| {
            let mortal = compare_immediate(b, NotEqual, count, IMMORTAL_REFCOUNT as u64);
            b.if_else(mortal, |b| b.slow_call(operation, bits), |_| {});
        },
    );
}

#[cfg(test)]
mod tests;
