//! Escape lattice and shared allocation/transparent-copy predicates.

use crate::tir::op_kinds_generated::opcode_is_escape_alloc_site_table;
use crate::tir::ops::OpCode;

/// Escape lattice for allocated values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EscapeState {
    /// No observed capture outside the function; does not establish storage or lifetime.
    NoEscape = 0,
    /// Passed across a call boundary with an explicit proof that no callee can capture it.
    ArgEscape = 1,
    /// Stored to heap/global or returned — must heap allocate.
    GlobalEscape = 2,
}

/// Returns `true` if this opcode is an allocation site whose result we
/// want to track for escape state.
///
/// * `Alloc` — generic heap blocks.
/// * `ObjectNewBound` — class-instance allocation from the frontend's
///   class-instantiation fold.
/// * `BuildList` / `BuildDict` / `BuildTuple` / `BuildSet` / `AllocTask` —
///   container / task allocation sites (S5 phase 1). Tracking these as escape
///   roots lets the alias analysis classify a freshly-built container's escape
///   state. This analysis does not rewrite allocation placement or ownership.
///   Consumers must independently prove callback, destruction, and representation
///   requirements before eliminating an allocation or reference operation.
#[inline]
pub(super) fn is_alloc_site(opcode: OpCode) -> bool {
    opcode_is_escape_alloc_site_table(opcode)
}
