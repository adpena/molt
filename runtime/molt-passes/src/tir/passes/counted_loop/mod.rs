//! Counted-loop recognition: the canonical counted-loop contract (L4).
//!
//! The frontend lowers `for i in range(start, stop, step):` to a counted
//! arithmetic loop with no iterator protocol op. This module recognizes that
//! real multi-arg SSA shape and exposes one [`CountedLoop`] descriptor consumed
//! by loop transforms and value-range reasoning.
//!
//! The recognizer refuses shapes outside its soundness boundary rather than
//! inventing trip counts: mismatched comparison polarity, non-constant starts or
//! steps, nested loops in the region, non-unique reachable preheaders, and
//! non-material exits where a transform requires an exit block.

mod descriptor;
mod facts;
mod gate;
mod recognize;
mod region;

pub use descriptor::CountedLoop;
pub use recognize::recognize_counted_loop;
pub(crate) use recognize::recognize_counted_loop_with_loop_forest;
pub use region::region_blocks;
