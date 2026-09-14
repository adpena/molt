//! Escape lattice and shared allocation/transparent-copy predicates.

use crate::tir::op_kinds_generated::{
    copy_kind_is_explicit_no_heap_move_table, opcode_is_escape_alloc_site_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode};

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

/// Extract a string attribute value from an `AttrDict`.
pub(super) fn attr_str<'a>(attrs: &'a AttrDict, key: &str) -> Option<&'a str> {
    match attrs.get(key) {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Returns `true` when an `OpCode::Copy` op is a genuine SSA move (its result
/// aliases its operand — the same heap object), as opposed to the opaque
/// `_original_kind` passthrough that `kind_to_opcode` assigns to SimpleIR ops
/// without a dedicated TIR opcode.
///
/// A move has either no `_original_kind` (a true SSA-lift copy) or an
/// `_original_kind` the generated registry proves is a no-heap move of operand 0
/// (the named SSA/var moves plus the validate-and-pass-through guards). Anything
/// else under `Copy` is a passthrough whose result is a *distinct* value (e.g. a
/// freshly built container), so it must NOT be aliased to its operand.
///
/// The kind set is the single generated authority `op_kinds.toml`
/// `classifier_no_heap_move` (`copy_kind_is_explicit_no_heap_move_table`), shared
/// with `alias_analysis.rs` and the ownership lattice — escape analysis no longer
/// keeps a private hand-list that could diverge. Every alias-propagation site
/// below reads `op.operands.first()` / `op.results.first()`, which matches the
/// registry's operand-0 pure-move contract for all those kinds (including the
/// guard passthroughs), so consuming the broader authority is sound and only
/// tightens the alias relation toward the rest of the compiler.
pub(super) fn is_pure_move_copy(attrs: &AttrDict) -> bool {
    match attr_str(attrs, "_original_kind") {
        None => true,
        Some(kind) => copy_kind_is_explicit_no_heap_move_table(kind),
    }
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
