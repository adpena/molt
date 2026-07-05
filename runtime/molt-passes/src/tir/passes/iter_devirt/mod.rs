//! List iterator devirtualization pass.
//!
//! Transforms `for x in some_list` from the iterator protocol into direct
//! index-based access, eliminating:
//!   - iterator object heap allocation (GetIter)
//!   - per-iteration `__next__` call + StopIteration check (IterNextUnboxed)
//!   - function call overhead for each element access
//!
//! Pattern matched (in TIR):
//! ```text
//!   iter_val  = GetIter(list_val)      // list_val known to be a list
//!   ...
//!   (elem, done) = IterNextUnboxed(iter_val)   // in loop header
//!   CondBranch(done, exit, body)
//! ```
//!
//! Transformed to:
//! ```text
//!   len_val = CallBuiltin("len", list_val)
//!   Branch -> header(0)
//!   header(i):
//!     cond = Lt(i, len_val)
//!     CondBranch(cond, body, exit)
//!   body:
//!     elem = Index(list_val, i)
//!     ... original body ...
//!     next_i = Add(i, 1)
//!     Branch -> header(next_i)
//! ```
//!
//! Detection: the source of `GetIter` is considered a list if:
//!   1. `TirFunction.value_types` records a `TirType::List(_)` fact, OR
//!   2. it was produced by a structural `BuildList` op, OR
//!   3. it is a list-repeat `Mul(BuildList, count)` chain.
//!
//! This runs early in the pipeline (after range_devirt, before type refinement)
//! so downstream passes can refine the index variable and element types.

mod candidate;
mod source;
mod transform;

#[cfg(test)]
mod tests;

use crate::tir::function::TirFunction;

use super::PassStats;
use candidate::find_candidates;
use transform::apply_transform;

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "iter_devirt",
        ..Default::default()
    };

    let candidates = find_candidates(func);
    if candidates.is_empty() {
        return stats;
    }

    for candidate in candidates {
        apply_transform(func, &candidate, &mut stats);
    }

    stats
}
