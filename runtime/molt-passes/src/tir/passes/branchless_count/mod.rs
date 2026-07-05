//! Branchless Boolean Counting Pass.
//!
//! Detects a diamond where the then arm increments a counter by one and the
//! else arm forwards the counter, then rewrites it to a single branchless add
//! in the condition block.

mod facts;
mod rewrite;

#[cfg(test)]
mod tests;

pub use rewrite::run;
