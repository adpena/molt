//! Per-call-site classification for CallFacts tables.
//!
//! The parent module owns table storage and cache registration. This module
//! owns the local and interprocedural facts computed for each call op.

use std::collections::BTreeMap;

use super::{CallFacts, CallTargetFact, FactValue, InlineEligibility};
use crate::repr::Repr;
use crate::tir::call_graph::CallGraph;
use crate::tir::call_targets::{direct_call_symbol_for_op, gpu_runtime_result_type_for_op};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{CallOpcodeRole, opcode_call_role_table};
use crate::tir::ops::TirOp;
use crate::tir::passes::inliner::classify_inline_eligibility;
use crate::tir::passes::ip_summary::ModuleSummaries;
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

// ───────────────────────────────────────────────────────────────────────────
// Per-call-site analysis
// ───────────────────────────────────────────────────────────────────────────

/// The result `ValueId` of a call-bearing op whose generated [`CallOpcodeRole`]
/// records facts, if it produces a value. Returns `None` for non-call ops and
/// for a (rare) result-less call. The key the side-table uses.
pub(super) fn call_op_result(op: &TirOp) -> Option<ValueId> {
    if !call_role_records_facts(opcode_call_role_table(op.opcode)) {
        return None;
    }
    op.results.first().copied()
}

/// Whether this generated call role is one CallFacts records.
#[inline]
fn call_role_records_facts(role: CallOpcodeRole) -> bool {
    matches!(
        role,
        CallOpcodeRole::UserCall | CallOpcodeRole::DynamicMethod | CallOpcodeRole::RuntimeBuiltin
    )
}

/// The typed return `Repr` for a call op's result, derived from the result
/// `ValueId`'s `TirType` in `func.value_types`. `Some(repr)` when the type is
/// known (non-`DynBox`); `None` when the semantic type is unknown. A known
/// semantic type can still require the `DynBox` carrier. [`Repr::default_for`] maps a
/// `TirType` to its conservative carrier — Phase 1 reports that floor (e.g.
/// `I64 → MaybeBigInt`, `F64/Bool → DynBox`), never exact scalar provenance.
/// The value-range / unboxing passes raise it later, and a
/// future coverage join over `typed_repr_report` reads the *post-pass* repr.
fn typed_return_for(result: ValueId, func: &TirFunction) -> Option<Repr> {
    match func.value_types.get(&result) {
        Some(TirType::DynBox) | None => None,
        Some(ty) => Some(Repr::default_for(ty)),
    }
}

/// The typed call target for a `Call` op, resolved against the module's defined
/// function set. `StaticDirect` iff the `Call` has a proven direct source role
/// naming a defined, non-gpu-runtime function; else `Opaque`. Dynamic-method opcodes and
/// `CallBuiltin` (runtime helper) are always `Opaque`. This mirrors
/// `call_graph::classify_call_op` exactly — same operation-aware call identity
/// and defined predicate — but returns the *typed* fact rather than a `CallEdge`.
fn target_for_module(op: &TirOp, call_graph: &CallGraph) -> CallTargetFact {
    match opcode_call_role_table(op.opcode) {
        CallOpcodeRole::UserCall => match direct_call_symbol_for_op(op) {
            // A gpu_* runtime symbol lifts to `Call` but is a runtime helper, not
            // a user function — the call graph excludes it as an edge, so it is
            // not a static-direct user target here either.
            Some(_) if gpu_runtime_result_type_for_op(op).is_some() => CallTargetFact::Opaque,
            Some(name) if call_graph.is_defined(name) => CallTargetFact::StaticDirect {
                callee: name.to_string(),
            },
            _ => CallTargetFact::Opaque,
        },
        // Method dispatch is always dynamic; a builtin is always a runtime helper.
        CallOpcodeRole::DynamicMethod | CallOpcodeRole::RuntimeBuiltin => CallTargetFact::Opaque,
        CallOpcodeRole::CopyOriginalKind | CallOpcodeRole::NotCall => CallTargetFact::Opaque,
    }
}

/// Compute the precise [`CallFacts`] for one call op, using the whole-program
/// context. The interprocedural path.
pub(super) fn analyze_call_site_module(
    op: &TirOp,
    func: &TirFunction,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
    by_name: &BTreeMap<&str, &TirFunction>,
) -> CallFacts {
    let result = op
        .results
        .first()
        .copied()
        .expect("analyze_call_site_module called on a result-less op");

    let target = target_for_module(op, call_graph);
    let typed_return = typed_return_for(result, func);

    // leaf / inlinable are callee-side: resolved only
    // for a StaticDirect target whose body is in this module.
    let resolved_callee: Option<&TirFunction> = target
        .static_callee()
        .and_then(|name| by_name.get(name).copied());

    // leaf: the resolved callee makes no call of any kind. `Proven` iff it is a
    // leaf, `False` iff it provably makes a call, `Unknown` if unresolved.
    let leaf = match target.static_callee() {
        Some(callee) => FactValue::from_decided(!call_graph.makes_any_call(callee)),
        None => FactValue::Unknown,
    };

    let no_throw = no_throw_for(op);

    // inlinable: the inliner's own decision (single source of truth). Only a
    // StaticDirect, module-resident callee is even a candidate; everything else
    // is `Unknown` (no body to gate against).
    let inlinable = match resolved_callee {
        Some(callee) => classify_inline_eligibility(callee, call_graph, summaries, tti),
        None => InlineEligibility::Unknown,
    };

    CallFacts {
        target,
        typed_return,
        leaf,
        no_throw,
        // Phase 2 (escape analysis). Fail-closed until then.
        no_alloc: FactValue::Unknown,
        inlinable,
    }
}

/// Compute the fail-closed intraprocedural floor [`CallFacts`] for one call op
/// (no module context). The [`Analysis::compute`] path.
pub(super) fn analyze_call_site_local(op: &TirOp, func: &TirFunction) -> CallFacts {
    let result = op
        .results
        .first()
        .copied()
        .expect("analyze_call_site_local called on a result-less op");

    // Without `defined`, a named `Call` target cannot be confirmed module-local,
    // so the target floors to `Opaque` (fail-closed: never claim StaticDirect we
    // cannot prove).
    let typed_return = typed_return_for(result, func);

    let no_throw = no_throw_for(op);

    CallFacts {
        target: CallTargetFact::Opaque,
        typed_return,
        leaf: FactValue::Unknown,
        no_throw,
        no_alloc: FactValue::Unknown,
        inlinable: InlineEligibility::Unknown,
    }
}

/// The generated operation contract is the no-throw authority for both local
/// and module tables. Absence of a handler does not prove absence of a raise;
/// even an innocent callee body does not prove call admission cannot fail.
/// Likewise a builtin name alone proves neither argument validity nor its
/// allocation/dispatch behavior. A may-throw contract yields `Unknown`, not
/// `False`: it permits a raise but does not prove this execution raises.
fn no_throw_for(op: &TirOp) -> FactValue {
    if !crate::tir::op_kinds_generated::opcode_may_throw_table(op.opcode) {
        return FactValue::Proven;
    }
    FactValue::Unknown
}
