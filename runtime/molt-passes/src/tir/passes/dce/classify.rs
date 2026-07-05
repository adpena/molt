use crate::tir::ops::TirOp;

#[cfg(test)]
use crate::tir::ops::OpCode;

use super::super::effects::op_has_observable_effect_when_dead;
#[cfg(test)]
use super::super::effects::opcode_may_throw;

/// Returns `true` if an op must be preserved even when all of its results
/// are dead.  Potential exceptions are observable control flow even outside
/// an explicit try region, so raising operations are side-effecting for DCE.
/// Checks both the opcode and, for `Copy` ops that originated from an unknown
/// SimpleIR kind, the `_original_kind` attribute so that unmapped call variants
/// are never silently dropped.
#[inline]
pub(super) fn op_is_side_effecting(op: &TirOp) -> bool {
    op_has_observable_effect_when_dead(op)
}

/// Returns `true` if the op may throw an exception.  Used by DCE to preserve
/// observable exceptional control flow and by `check_exception_elim` to avoid
/// removing required checks.
///
/// Opcode-level query kept for tests and coarse callers. Op-instance effect
/// proofs are handled by the central effects oracle before DCE weakens
/// observable semantics.
#[cfg(test)]
#[inline]
pub(super) fn is_potentially_throwing(opcode: OpCode) -> bool {
    opcode_may_throw(opcode)
}
