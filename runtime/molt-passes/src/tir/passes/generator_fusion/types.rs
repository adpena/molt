use super::super::super::blocks::BlockId;
use super::super::super::values::ValueId;

/// Statistics from one [`run_generator_fusion`] invocation over a module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FusionStats {
    /// Number of generator frames elided (one per successful splice).
    pub frames_elided: usize,
    /// Number of yield sites spliced into consumer bodies.
    pub yield_sites_spliced: usize,
    /// Names of the consumer functions whose body was changed by fusion (a
    /// generator was spliced in). Production codegen must back-convert /
    /// re-lower ONLY these functions' (post-fusion) TIR — the module phase folds
    /// this into its `changed_functions` set exactly as it does the inliner's.
    pub changed_functions: Vec<String>,
}

/// A recognized fusion candidate: an `AllocTask(generator)` consumed by a single
/// `GetIter` → single `IterNext`-loop in `caller`.
pub(in crate::tir::passes::generator_fusion) struct FusionCandidate {
    /// Block + op index of the `AllocTask` in the caller.
    pub(in crate::tir::passes::generator_fusion) alloc_block: BlockId,
    pub(in crate::tir::passes::generator_fusion) alloc_idx: usize,
    /// The generator frame value produced by `AllocTask`.
    pub(in crate::tir::passes::generator_fusion) alloc_val: ValueId,
    /// The `_poll` function name (a module-defined function).
    pub(in crate::tir::passes::generator_fusion) poll_name: String,
    /// Block holding the `GetIter` (or `iter` Copy) in the caller.
    pub(in crate::tir::passes::generator_fusion) get_iter_block: BlockId,
    /// The iterator value produced by `GetIter`.
    pub(in crate::tir::passes::generator_fusion) iter_val: ValueId,
    /// The loop-condition block holding the `IterNext` + done-check.
    pub(in crate::tir::passes::generator_fusion) cond_block: BlockId,
    /// The `(value, done)` pair value produced by `IterNext`.
    pub(in crate::tir::passes::generator_fusion) pair_val: ValueId,
    /// The block holding the `Index(pair, 0)` element-extraction (the body block,
    /// or the cond block if the element is extracted before the branch).
    pub(in crate::tir::passes::generator_fusion) elem_block: BlockId,
    /// The element value (`pair[0]`).
    pub(in crate::tir::passes::generator_fusion) elem_val: ValueId,
    /// The block control branches to on `done == true` (loop exit) and
    /// `done == false` (loop body).
    pub(in crate::tir::passes::generator_fusion) exit_block: BlockId,
    pub(in crate::tir::passes::generator_fusion) body_block: BlockId,
    /// The loop header (the `LoopHeader`-role block that targets `cond_block`).
    /// Present iff the consumer carries structured loop metadata.
    pub(in crate::tir::passes::generator_fusion) loop_header: Option<BlockId>,
}

/// A user frame slot's resolved promotion data.
pub(in crate::tir::passes::generator_fusion) struct SlotInfo {
    /// Frame byte offset (`>= GEN_CONTROL_BYTES`).
    pub(in crate::tir::passes::generator_fusion) offset: i64,
    /// The preheader init value, expressed in the CALLER's value space (a clone
    /// of the AllocTask arg for a param slot, or a fresh clone of the poll's
    /// entry init for a local slot, or a fresh `None` for an unwritten slot).
    pub(in crate::tir::passes::generator_fusion) init_caller_val: ValueId,
}
