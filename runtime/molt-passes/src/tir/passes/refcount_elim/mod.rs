//! Reference-count elimination for TIR.
//!
//! Elide proven non-heap and explicit stack-object reference counts and cancel a retain followed by
//! its matching release across callback-free, exception-free execution.
//! Cross-block cancellation additionally requires one unconditional edge into
//! a successor with exactly one predecessor. The local algorithm handles loop
//! headers too; no separate loop scan owns the same pairing rule.
//!
//! NoEscape proves frame lifetime, not absence of destruction obligations.
//! Heap-backed locals retain their final release, including contained objects
//! and finalizers. A release followed by a retain is not a cancellable pair:
//! the release can cross zero. This pass never synthesizes direct Free.

mod balance;
mod cross_block;
mod engine;
mod facts;
mod local;

pub use engine::{run, run_post_drop};
