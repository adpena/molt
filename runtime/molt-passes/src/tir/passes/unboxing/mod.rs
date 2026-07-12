//! Unboxing pass: eliminates redundant Box/Unbox pairs.
//!
//! When a value is boxed (`BoxVal`) and all consumers unbox it back to the same
//! type (`UnboxVal`), both operations are unnecessary. The original unboxed
//! value can be used directly.

mod engine;
mod terminator;

#[cfg(test)]
mod tests;

pub use engine::run;
