use crate::tir::blocks::BlockId;
use crate::tir::values::ValueId;

/// A recognized counted loop with a compile-time-constant trip count.
///
/// The induction variable and every loop-carried value are header block-args;
/// `iv_arg_index` selects the IV among them. A transform threads the carried
/// values (all indices `!= iv_arg_index`) through each iteration while the IV
/// takes the constant value `start + k*step` on iteration `k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountedLoop {
    /// The loop header block (a pure phi block: `Branch -> cond_block`).
    pub header: BlockId,
    /// The block holding the loop-exit comparison + `CondBranch(body, exit)`.
    /// Equal to `header` only in the legacy 1-arg synthesized shape where the
    /// comparison lives in the header itself.
    pub cond_block: BlockId,
    /// The loop body block (the `CondBranch` successor that loops back).
    pub body: BlockId,
    /// The exit block (the `CondBranch` successor that does not loop back).
    pub exit: BlockId,
    /// The unique reachable preheader (non-back-edge predecessor of `header`).
    pub preheader: BlockId,
    /// Index into `header.args` of the induction variable.
    pub iv_arg_index: usize,
    /// The induction-variable `ValueId` (`header.args[iv_arg_index].id`).
    pub induction_var: ValueId,
    /// Start value of the induction variable (preheader-provided constant).
    pub start: i64,
    /// Step per iteration (non-zero compile-time constant).
    pub step: i64,
    /// Trip count (number of iterations; always `> 0`).
    pub trip_count: i64,
    /// The exit-edge argument list on the cond block's `CondBranch` (the values
    /// forwarded to `exit`). May reference header args (loop-carried values) or
    /// the IV; a transform substitutes those with their final-iteration values.
    pub exit_args: Vec<ValueId>,
    /// True when `exit` is a real CFG successor of the loop guard. Terminal
    /// structured loops may preserve their break predicate in metadata even when
    /// there is no material post-loop block; range analysis can consume that
    /// proof, but transforms that must branch to an exit block must refuse it.
    pub has_material_exit: bool,
    /// The back-edge argument list on the body's `Branch -> header` (the values
    /// forwarded to the header for the next iteration). `back_args[k]` fills
    /// `header.args[k]`.
    pub back_args: Vec<ValueId>,
    /// The structural `LoopEnd` marker block paired with this header
    /// (`loop_pairs[header]`), if any. The frontend leaves this as an
    /// unreachable dead block; a transform that unrolls the loop away must drop
    /// its now-orphaned `LoopEnd` role so the TIR->SimpleIR back-conversion does
    /// not see a `LoopEnd` without a matching `LoopHeader`.
    pub loop_pairs_end: Option<BlockId>,
}
