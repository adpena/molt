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
    pub changed_functions: Vec<molt_ir::FunctionId>,
    /// Each generator poll body that may not be fused, by name, with the
    /// exclusion that refused it (reported under `MOLT_INLINE_STATS`).
    pub refused_polls: Vec<(String, super::PollRefusal)>,
}

/// A recognized fusion candidate: an `AllocTask(generator)` consumed by a single
/// `GetIter` and a single `IterNextUnboxed` loop in `caller`.
pub(in crate::tir::passes::generator_fusion) struct FusionCandidate {
    /// Block + op index of the `AllocTask` in the caller.
    pub(in crate::tir::passes::generator_fusion) alloc_block: BlockId,
    pub(in crate::tir::passes::generator_fusion) alloc_idx: usize,
    /// The generator frame value produced by `AllocTask`.
    pub(in crate::tir::passes::generator_fusion) alloc_val: ValueId,
    /// The generator function the `AllocTask` names: the fused activation's
    /// frame context, which its frame states carry.
    pub(in crate::tir::passes::generator_fusion) function: ValueId,
    /// The `_poll` function (a body in this module).
    pub(in crate::tir::passes::generator_fusion) poll: molt_ir::FunctionId,
    /// Block holding the `GetIter` (or `iter` Copy) in the caller.
    pub(in crate::tir::passes::generator_fusion) get_iter_block: BlockId,
    /// The iterator value produced by `GetIter`.
    pub(in crate::tir::passes::generator_fusion) iter_val: ValueId,
    /// The block holding the `IterNextUnboxed`.
    pub(in crate::tir::passes::generator_fusion) cond_block: BlockId,
    /// The element and done results of the `IterNextUnboxed`.
    pub(in crate::tir::passes::generator_fusion) elem_val: ValueId,
    /// The empty blocks the next's block branches through to the done test.
    pub(in crate::tir::passes::generator_fusion) chain: Vec<BlockId>,
    /// The handler of the iteration's Origin check: the caller block a
    /// pending exception of the generator body reaches. The fused body's
    /// exception exits branch to it.
    pub(in crate::tir::passes::generator_fusion) handler: BlockId,
    /// The values the iteration's check passes its handler.
    pub(in crate::tir::passes::generator_fusion) handler_args: Vec<ValueId>,
    /// The caller's location at the iteration: the call site the fused
    /// activation's frame hangs below.
    pub(in crate::tir::passes::generator_fusion) call_location: Option<i64>,
    /// The block control branches to on `done == true` (loop exit) and
    /// `done == false` (loop body).
    pub(in crate::tir::passes::generator_fusion) exit_block: BlockId,
    pub(in crate::tir::passes::generator_fusion) body_block: BlockId,
    /// The loop header (the `LoopHeader`-role block that targets `cond_block`).
    /// Present iff the consumer carries structured loop metadata.
    pub(in crate::tir::passes::generator_fusion) loop_header: Option<BlockId>,
}
