//! Static Basic Block Versioning (SBBV) pass for TIR.
//!
//! Based on the ECOOP 2024 paper, this pass eliminates runtime type guards by
//! duplicating blocks with different type assumptions. The caller dispatches
//! to the right version based on what it knows about the operand types.
//!
//! ## Algorithm
//!
//! 1. Scan each block for TypeGuard ops. Each guard on value `%x` proving type
//!    `T` forms a "type context" — a (ValueId, TirType) pair.
//!
//! 2. For each such block, create up to k=2 versions:
//!    - **Specialized version**: the TypeGuard is removed and all uses of its
//!      result within the block are replaced with a constant `true` (the guard
//!      is known to succeed). The guarded value's type is refined.
//!    - **Generic version**: the original block, unchanged.
//!
//! 3. Rewire predecessors: if a predecessor can statically prove the guard
//!    condition (e.g., the guarded value is a typed block argument or was
//!    produced by an opcode with an operand-independent result type), route it
//!    to the specialized version.
//!    Otherwise route to the generic version.
//!
//! 4. LoopForest loop headers are not versioned — doing so could create
//!    unbounded versioning or violate SSA dominance on loop-carried values.
//!
//! 5. After versioning, blocks whose predecessors ALL route to the specialized
//!    version leave the generic version unreachable. The DCE pass (run later
//!    in the pipeline) will clean it up.
//!
//! ## Bounded code size
//!
//! The k=2 limit ensures at most 2x code size increase. In practice, most
//! blocks have at most one TypeGuard, so the increase is much smaller.

mod analysis;
mod block_clone;
mod runner;

#[cfg(test)]
mod tests;

pub use runner::run;
