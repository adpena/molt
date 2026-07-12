//! Refcount Elimination pass for TIR.
//!
//! Eliminates redundant IncRef/DecRef pairs both within and across basic blocks.
//!
//! Intra-block patterns:
//! 1. Adjacent: IncRef(x); DecRef(x) â†’ both removed
//! 2. Reversed: DecRef(x); IncRef(x) â†’ both removed (ownership transfer)
//! 3. NoEscape: IncRef/DecRef on values classified as StackAlloc â†’ removed
//!    (escape analysis already rewrote Allocâ†’StackAlloc, this catches remaining refs)
//!
//! Cross-block patterns:
//! 4. Dominator edge: block A dominates block B, A is B's sole predecessor,
//!    A ends with IncRef(x) (no trailing barrier), B starts with DecRef(x)
//!    (no leading barrier) â†’ both removed. The paired IncRef created the extra
//!    ref that the DecRef destroys, so eliminating both is safe.
//! 5. Loop invariant: loop header has IncRef(x) at top and DecRef(x) at bottom
//!    (before back-edge), x is loop-invariant (defined outside the loop body),
//!    and no barrier intervenes between them within the header â†’ both removed.
//!
//! Deferred RC (Deutsch-Bobrow 1976):
//! 6. Only track references from HEAP objects. Stack/register references are
//!    implicitly alive during their scope. Values with no "heap exposure"
//!    (never passed to calls, returned, stored to attrs/indices/closures,
//!    yielded, raised, or placed into containers) have all IncRef/DecRef
//!    eliminated unconditionally.

mod balance;
mod cross_block;
mod deferred;
mod engine;
mod facts;
mod local;
mod loops;

#[cfg(test)]
mod tests;

pub use engine::{run, run_post_drop};
