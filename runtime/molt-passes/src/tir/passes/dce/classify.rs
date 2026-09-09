#[cfg(test)]
use crate::tir::ops::OpCode;

#[cfg(test)]
use super::super::effects::opcode_may_throw;

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
