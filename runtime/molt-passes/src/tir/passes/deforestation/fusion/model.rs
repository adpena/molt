use crate::tir::blocks::BlockId;
use crate::tir::op_kinds_generated::opcode_is_fusion_barrier_table;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

/// Recognized builtin consumer that can be fused with an iterator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FusableBuiltin {
    Sum,
    Any,
    All,
    Min,
    Max,
    List,
    /// `len(iterable)` → counter loop (no intermediate list allocation).
    Len,
    /// `set(iterable)` → direct set-build loop (no intermediate list).
    Set,
    /// `tuple(iterable)` → direct tuple build (no intermediate list).
    Tuple,
    /// `sorted(iterable)` → collect + sort-in-place (single allocation).
    Sorted,
    /// `reversed(iterable)` → reverse-iteration (no materialized copy).
    Reversed,
}

impl FusableBuiltin {
    /// Try to parse a builtin name from a `CallBuiltin` attribute.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "sum" => Some(Self::Sum),
            "any" => Some(Self::Any),
            "all" => Some(Self::All),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            "list" => Some(Self::List),
            "len" => Some(Self::Len),
            "set" => Some(Self::Set),
            "tuple" => Some(Self::Tuple),
            "sorted" => Some(Self::Sorted),
            "reversed" => Some(Self::Reversed),
            _ => None,
        }
    }
}

/// Description of a `GetIter` → `ForIter` loop feeding into a `CallBuiltin`.
#[derive(Debug)]
pub(super) struct IteratorChain {
    /// Block containing the `CallBuiltin` consumer.
    pub(super) consumer_block: BlockId,
    /// Index of the `CallBuiltin` op within its block.
    pub(super) consumer_op_idx: usize,
    /// Which builtin is consuming the iterator.
    pub(super) builtin: FusableBuiltin,
    /// Block containing the `ForIter` loop header.
    pub(super) loop_header_block: BlockId,
    /// Index of the `ForIter` op within the loop header block.
    pub(super) for_iter_op_idx: usize,
    /// Block containing the loop body ops.
    pub(super) loop_body_block: BlockId,
    /// The `ValueId` produced by `GetIter` (the iterator object).
    #[allow(dead_code)]
    pub(super) iter_value: ValueId,
    /// The `ValueId` produced by `IterNext`/`ForIter` (each element).
    pub(super) element_value: ValueId,
    /// The `ValueId` that the `CallBuiltin` produces (the result).
    pub(super) result_value: ValueId,
    /// The iterable source passed to `GetIter`.
    pub(super) source_iterable: ValueId,
}

/// Returns `true` when an opcode blocks iterator-chain fusion.
///
/// The canonical, exhaustive table lives in `op_kinds.toml`.
fn is_fusion_barrier(opcode: OpCode) -> bool {
    opcode_is_fusion_barrier_table(opcode)
}

/// Check whether every op in a slice is eligible for iterator-chain fusion.
pub(in crate::tir::passes::deforestation) fn is_fusable_body(ops: &[TirOp]) -> bool {
    ops.iter().all(|op| !is_fusion_barrier(op.opcode))
}
