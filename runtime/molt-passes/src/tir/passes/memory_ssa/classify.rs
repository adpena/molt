use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::super::alias_analysis::{AliasAnalysisResult, LoadPurity, MemRegion};

// ===========================================================================
// Op classification — derived ENTIRELY from the alias oracle (no duplication)
// ===========================================================================

/// How an op participates in MemorySSA, decided purely from the public alias
/// oracle queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MemRole {
    /// A clobbering write (`StoreAttr`, `StoreIndex`, a call/raise/yield
    /// barrier, a module mutation, or a `MayDispatch` load that may run a user
    /// dunder writing arbitrary memory). Produces a new memory version.
    Def,
    /// A proven-pure typed-slot load. Reads a memory version; defines none.
    Use,
    /// Touches no heap memory (a pure register computation, constant, control
    /// marker). Not part of the memory graph.
    None,
}

/// Classify an op's memory role using ONLY [`AliasAnalysisResult`]'s public
/// surface — the single source of truth for memory-aliasing facts.
///
/// * `ScalarRegister` region ⇒ [`MemRole::None`] (no heap footprint).
/// * A proven-pure load (`load_purity == ProvenPure`, i.e. a typed-slot
///   `LoadAttr`) ⇒ [`MemRole::Use`].
/// * Everything else touching non-scalar memory ⇒ [`MemRole::Def`] (a
///   conservative clobber). This subsumes every heap-barrier opcode — the alias
///   oracle already widens calls/raises/yields/module-mutations to `GenericHeap`
///   — and every `MayDispatch` load, which may dispatch a writing dunder.
pub(super) fn classify(op: &TirOp, alias: &AliasAnalysisResult) -> MemRole {
    let region = alias.region_of(op);
    if region == MemRegion::ScalarRegister {
        return MemRole::None;
    }
    // A load (`LoadAttr` / `Index`) that is proven-pure reads but never writes.
    // Every other non-scalar op is a clobbering def. `load_purity` only returns
    // `ProvenPure` for typed-slot `LoadAttr` ops, so this gate is exactly the
    // "pure read" set.
    if matches!(op.opcode, OpCode::LoadAttr | OpCode::Index)
        && alias.load_purity(op) == LoadPurity::ProvenPure
    {
        MemRole::Use
    } else {
        MemRole::Def
    }
}

/// `Some((target, stored_value, offset))` for the narrow typed-slot store
/// (`store` / `store_init`) — used to expose the forwarded value to consumers.
/// Operands are `[target, stored_value]`; the offset is the `value` attr.
pub fn typed_slot_store_value(op: &TirOp) -> Option<(ValueId, ValueId, i64)> {
    if op.opcode != OpCode::StoreAttr || op.operands.len() != 2 {
        return None;
    }
    let kind = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(kind, "store" | "store_init") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(offset)) => Some((op.operands[0], op.operands[1], *offset)),
        _ => None,
    }
}
