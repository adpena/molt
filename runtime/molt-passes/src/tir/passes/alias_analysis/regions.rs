//! Physical memory-region taxonomy and the barrier opcode core.
//!
//! The [`MemRegion`] partition, the typed-slot field footprints, and the opcode-only
//! RC/heap barrier predicates. Split out of `alias_analysis.rs` as a move-only
//! decomposition; the union-find, borrow provenance, and cached
//! [`super::AliasAnalysisResult`] consume these via the parent module.

use crate::tir::op_kinds_generated::opcode_is_alias_rc_barrier_table;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

// ===========================================================================
// MemRegion taxonomy
// ===========================================================================

/// Physical memory footprint. Source class names and static annotations are
/// not allocation identities: inherited views can name the same boxed word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemRegion {
    /// A direct boxed-word access. An allocation identity is present only for
    /// an exact allocation-result alias, never a may-alias CFG parameter.
    /// None may name any allocation, including one also accessed precisely.
    Field {
        allocation: Option<ValueId>,
        offset: i64,
    },
    /// Opaque container storage; dispatching operations widen to GenericHeap.
    ContainerElement,
    /// Globally visible module dictionary storage.
    ModuleDict,
    /// The whole fresh storage written by a callback-free local allocation.
    LocalAllocation { root: ValueId },
    /// A register with no heap footprint.
    ScalarRegister,
    /// Unknown or callback-capable access, aliasing every heap region.
    GenericHeap,
}

impl MemRegion {
    /// Disjointness requires different known allocations or nonoverlapping
    /// physical words. Capture does not change identity, and class spelling
    /// never proves identity. Callback/destruction effects remain GenericHeap.
    pub fn may_alias(&self, other: &MemRegion) -> bool {
        use MemRegion::*;
        match (self, other) {
            (ScalarRegister, _) | (_, ScalarRegister) => false,
            (GenericHeap, _) | (_, GenericHeap) => true,
            (
                Field {
                    allocation: a,
                    offset: x,
                },
                Field {
                    allocation: b,
                    offset: y,
                },
            ) => allocations_may_alias(*a, *b) && boxed_words_overlap(*x, *y),
            (LocalAllocation { root: a }, LocalAllocation { root: b }) => a == b,
            (LocalAllocation { root }, Field { allocation, .. })
            | (Field { allocation, .. }, LocalAllocation { root }) => {
                allocations_may_alias(Some(*root), *allocation)
            }
            (Field { .. } | LocalAllocation { .. }, _)
            | (_, Field { .. } | LocalAllocation { .. }) => false,
            (ContainerElement, ModuleDict) | (ModuleDict, ContainerElement) => false,
            _ => true,
        }
    }
}

fn allocations_may_alias(left: Option<ValueId>, right: Option<ValueId>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn boxed_words_overlap(left: i64, right: i64) -> bool {
    left.abs_diff(right) < std::mem::size_of::<u64>() as u64
}

/// `Some((object, offset))` for a direct typed-slot load or store. The shared
/// `TirOp` projections own every shape/kind/offset check so alias, MemorySSA,
/// MemGVN, and DSE cannot drift. Guarded field operations are deliberately
/// excluded: their runtime miss paths invoke generic attribute lookup/mutation.
pub(super) fn typed_slot_obj_offset(op: &TirOp) -> Option<(ValueId, i64)> {
    op.plain_typed_slot_load()
        .or_else(|| op.plain_typed_slot_store())
}

// ===========================================================================
// The barrier core — conservative superset of all four old lists
// ===========================================================================

/// The opcode-only "could this op capture/store/observe a reference count"
/// predicate. This is the EXACT superset core that `refcount_elim::is_barrier`
/// required, plus the additional ops that only ever *add* barriers (it is sound
/// to over-barrier RC pairing). Operand-agnostic by design: an RC barrier blocks
/// pairing regardless of which value the op touches.
///
/// Superset obligation vs the old `refcount_elim::is_barrier`: every opcode in
/// that list ({Call, CallMethod, CallBuiltin, StoreAttr, StoreIndex, StateSwitch,
/// StateTransition, StateYield, ClosureLoad, ClosureStore, ChanSendYield,
/// ChanRecvYield}) is present here. Exception-control transfer is also a
/// barrier: `Raise` does not fall through, and `CheckException` / `TryStart`
/// carry implicit handler edges whose payload retains are consumed only on that
/// exceptional path. Verified in `tests::rc_barrier_is_superset_*`.
pub(super) fn opcode_is_rc_barrier(opcode: OpCode) -> bool {
    opcode_is_alias_rc_barrier_table(opcode)
}
