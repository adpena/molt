//! Destruction-preserving elimination of unobserved plain typed-slot stores.
//!
//! A field write is not just a memory update: `store` retains an incoming heap
//! value and releases the displaced value, whose finalizer can observe or mutate
//! any captured object. Materialized instance dictionaries own their field state.
//! A store to a proven pristine, release-neutral old slot performs
//! no old release, but that is an analysis fact rather than an opcode spelling.
//! Neither an unread field nor a noescape object makes these ownership operations
//! erasable.
//!
//! Overwrite elimination requires:
//!
//! - The receiver is a fresh fixed-layout allocation on an unconditional chain that has
//!   never crossed an observation/capture boundary. This history is permanent;
//!   repopulating pending stores after a callback cannot restore freshness.
//! - The incoming value is proven nonheap in its **boxed field representation**
//!   by the shared representation/range authority. A raw full-i64 carrier may
//!   box to heap BigInt and is not enough.
//! - The old field is raw boxed zero, class missing from a validated allocation, or this
//!   exact alias-root/offset contains a proven boxed-neutral value. Field bounds
//!   exclude the reserved trailing instance-dictionary word.
//! - The store has no result definitions that its removal would orphan.
//!
//! The shared typed_slot_access analysis carries both the current-value fact
//! and removable-store index for this pass and backend field lowering. The shared alias oracle invalidates
//! it for callbacks,
//! reads, unknown effects, and escapes, including operations without the receiver
//! among their operands. Non-neutral overwrites cannot establish a new fact:
//! their release callback could reenter and replace the just-written value.
//!
//! A later admitted write to the same slot makes its predecessor dead. Final
//! writes always survive: absent finalizer metadata does not prove destruction
//! unobservable, and mutable classes can acquire a finalizer after construction.
//! Only SROA's
//! complete callback-free whole-object proof may remove a final field together
//! with the allocation and all ownership operations.
//!
//! Scalar constructor initialization/overwrite chains stay optimizable. Class
//! names do not separate inherited slots; heap values, unknown old contents,
//! and observed/captured roots stay live.
//! Output is compacted in one stable linear pass. `PassStats.ops_removed` counts
//! the removed stores.

mod rewrite;

use crate::tir::analysis::AnalysisManager;
use crate::tir::function::TirFunction;
use crate::tir::passes::PassStats;

/// Public entry point - run dead-store elimination on every block.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    rewrite::run(func, am)
}
