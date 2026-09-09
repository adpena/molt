//! Branchless Boolean Counting Pass.
//!
//! Detects a diamond where the then arm increments a counter by one and the
//! else arm forwards the counter, then rewrites it to a single branchless add
//! in the condition block.
//!
//! The condition must be an exact boolean from the shared scalar authority,
//! not a signature annotation. The counter and incremented endpoint must fit
//! the nonallocating inline-int range: evaluating addition on the false path
//! may not dispatch callbacks, allocate a bigint, or change object identity.
//! Both removed arms must belong exclusively to this diamond, with no implicit
//! exception entry or structural label/loop owner. Shared function block
//! retirement removes their metadata in one batch. Value-range analysis is lazy
//! and shared across admitted candidates in the function.

mod facts;
mod rewrite;

#[cfg(test)]
mod tests;

pub use rewrite::run;
