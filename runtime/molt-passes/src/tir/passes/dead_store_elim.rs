//! Dead-store elimination for `StoreAttr` ops within a single basic block.
//!
//! Pattern 1: when two `StoreAttr` ops within the same block target the
//! same object value at the same offset and there is no intervening read
//! or escape of that object, the earlier store is dead and can be removed.
//!
//! Pattern 2: when the final stores to a typed-class instance target an
//! `ObjectNewBoundStack` value allocated in the same block, AND that stack
//! object is provably unobservable outside the block, those stores are also
//! dead.  "Unobservable outside the block" requires BOTH that the object is not
//! used by the terminator AND that its alias root is not referenced in any other
//! block (see `compute_escaping_roots`): TIR's SSA admits *dominance-based*
//! cross-block uses, so a value can be read in a later block without appearing in
//! this block's terminator arguments.  Checking the terminator alone is unsound:
//! it dropped a constructor's field stores whenever a CFG split (try/except, or
//! any branch) separated the construction from a later field read, yielding a
//! silent zero-default field read (Task #20 P0).  Any intervening observer within
//! the block still invalidates the pending-store state below.
//!
//! The most common producer of this pattern is the frontend's class-
//! instantiation fold combined with the `__init__` inliner: the inlined
//! `__init__` body emits `store_init` for each declared field with the
//! constructor's default value, then user code immediately overwrites the
//! same fields with non-default values:
//!
//! ```text
//! object_new_bound_stack out=_v23 args=[cls] value=24
//! store_init args=[_v23, _v_zero] value=0   ; p.x = 0  (init)
//! store_init args=[_v23, _v_zero] value=8   ; p.y = 0  (init)
//! store args=[_v23, _v_i] value=0           ; p.x = i  (overwrite - kills the init)
//! store args=[_v23, _v_iplus1] value=8      ; p.y = i+1
//! ```
//!
//! The two `store_init` ops are dead in this loop body.  Eliminating them
//! drops 2 stores per typed-class instance in the hot loop.
//!
//! ## Soundness
//!
//! A store `S1[obj, *] offset=N` is dead iff, walking forward from S1
//! within the same basic block, we encounter another typed-slot store
//! `S2[obj_or_alias, *] offset=N` BEFORE any of:
//!   - a read of `obj` or one of its transparent aliases (`LoadAttr`,
//!     indexed access, or any op that could observe the slot's value),
//!   - an escape of `obj` (`Call`, `CallMethod`, `CallBuiltin`, `Raise`,
//!     yielding, storing it into another object/container, etc.),
//!   - a control-flow boundary (we restrict the analysis to a single
//!     block - cross-block dead-store would need full alias analysis).
//!
//! When all conditions hold, S1's writes are unobservable: the slot is
//! only read AFTER S2, which provides a fresh value.
//!
//! ### Key conservatism
//!
//! - Any op whose operand list contains `obj` or a tracked transparent
//!   alias and whose effects we don't recognize is treated as a possible
//!   read or escape => S1 stays live.
//! - We scope the forward overwrite walk to a single block: dead stores across
//!   blocks are left live unless overwritten before the block ends. Cross-block
//!   elimination belongs in a full memory dataflow pass with alias facts.
//! - Pattern 2's "object confined to this block" precondition is, by contrast,
//!   a WHOLE-FUNCTION fact (`compute_escaping_roots`): a stack object whose
//!   pointer is referenced in any other block is observable downstream, so its
//!   final stores stay live.
//! - Stores with no resolvable offset attr stay live.
//! - Only `StoreAttr` ops with `_original_kind in {"store", "store_init"}`
//!   are considered - other StoreAttr variants (set_attr_name,
//!   guarded_field_set, etc.) have different operand conventions and effects
//!   and are out of scope. Module attribute mutation is represented by the
//!   first-class `ModuleSetAttr` opcode, not StoreAttr transport.
//!
//! ## Statistics
//!
//! Returns the number of dead stores removed via `PassStats.ops_removed`.

mod access;
mod escape;
mod rewrite;
#[cfg(test)]
mod tests;

use crate::tir::analysis::AnalysisManager;
use crate::tir::function::TirFunction;
use crate::tir::passes::PassStats;

/// Public entry point - run dead-store elimination on every block.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    rewrite::run(func, am)
}
