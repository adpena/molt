//! Per-call-site classification for CallFacts tables.
//!
//! The parent module owns table storage and cache registration. This module
//! owns the local and interprocedural facts computed for each call op.

use std::collections::BTreeMap;

use super::{CallFacts, CallTargetFact, FactValue, InlineEligibility};
use crate::repr::Repr;
use crate::tir::call_graph::CallGraph;
use crate::tir::call_targets::is_gpu_runtime_symbol;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{CallOpcodeRole, opcode_call_role_table};
use crate::tir::ops::{AttrValue, TirOp};
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

/// Read an op's `s_value` string attr (the `Call` callee name), if present.
fn s_value(op: &TirOp) -> Option<&str> {
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Read a `CallBuiltin`'s builtin name. The SSA lift stores it under the `name`
/// attr key (not `s_value`); `range_new` is normalized to `name = "range"`.
fn builtin_name(op: &TirOp) -> Option<&str> {
    match op.attrs.get("name") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// The typed return `Repr` for a call op's result, derived from the result
/// `ValueId`'s `TirType` in `func.value_types`. `Some(repr)` when the type is
/// precise (non-`DynBox`); `None` when `DynBox` (the boxed universal carrier) or
/// when the type is unknown. The lattice floor [`Repr::default_for`] maps a
/// `TirType` to its conservative carrier — Phase 1 reports that floor (e.g.
/// `I64 → MaybeBigInt`); the value-range / unboxing passes raise it later, and a
/// future coverage join over `typed_repr_report` reads the *post-pass* repr.
fn typed_return_for(result: ValueId, func: &TirFunction) -> Option<Repr> {
    match func.value_types.get(&result) {
        Some(TirType::DynBox) | None => None,
        Some(ty) => Some(Repr::default_for(ty)),
    }
}

/// Builtins that provably cannot raise for *any* arguments — the Phase-1 no-throw
/// allowlist (doc 47 §2). Conservative: only pure, total builtins whose molt
/// runtime implementation has no error path for valid (already type-checked)
/// operands. A builtin not on this list is `Unknown` (fail-closed), never
/// asserted no-throw. `len` is intentionally EXCLUDED — it dispatches `__len__`,
/// which can raise.
fn builtin_is_no_throw(name: &str) -> bool {
    matches!(
        name,
        // Identity / introspection on an already-realized object: no dispatch,
        // no allocation failure path that surfaces as a Python exception.
        "id" | "type" | "is" | "isinstance_fast"
    )
}

/// The typed call target for a `Call` op, resolved against the module's defined
/// function set. `StaticDirect` iff the `Call`'s `s_value` names a defined,
/// non-gpu-runtime function; else `Opaque`. Dynamic-method opcodes and
/// `CallBuiltin` (runtime helper) are always `Opaque`. This mirrors
/// `call_graph::classify_call_op` exactly — same `s_value`/defined predicate, same
/// gpu-runtime carve-out — but returns the *typed* fact rather than a `CallEdge`.
fn target_for_module(op: &TirOp, call_graph: &CallGraph) -> CallTargetFact {
    match opcode_call_role_table(op.opcode) {
        CallOpcodeRole::UserCall => match s_value(op) {
            // A gpu_* runtime symbol lifts to `Call` but is a runtime helper, not
            // a user function — the call graph excludes it as an edge, so it is
            // not a static-direct user target here either.
            Some(name) if is_gpu_runtime_symbol(name) => CallTargetFact::Opaque,
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

    // leaf / inlinable / callee-handler no_throw are callee-side: resolved only
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

    // no_throw: opcode statically no-throw ∨ resolved callee has no handlers ∨
    // a no-throw-allowlisted builtin. Else Unknown (fail-closed).
    let no_throw = no_throw_for(op, resolved_callee);

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

    // no_throw: only the *locally* decidable halves — a statically-no-throw
    // opcode (none of the call opcodes are, but a future opcode might be) or a
    // no-throw builtin. The callee-has-no-handlers half needs the body, so it is
    // omitted here (yields `Unknown`, not a false claim).
    let no_throw = no_throw_for(op, None);

    CallFacts {
        target: CallTargetFact::Opaque,
        typed_return,
        leaf: FactValue::Unknown,
        no_throw,
        no_alloc: FactValue::Unknown,
        inlinable: InlineEligibility::Unknown,
    }
}

/// The Phase-1 `no_throw` skeleton (doc 47 §2). `Proven` iff:
///   1. the opcode is statically no-throw (per the generated `op_kinds` registry —
///      the authoritative effect oracle, read never re-decided, doc 47 §7), OR
///   2. `resolved_callee` is `Some` and has no exception **handler** region
///      (`TirFunction::has_exception_handlers` — a callee that cannot itself
///      enter a handler cannot raise *through* one on this edge), OR
///   3. the op is a `CallBuiltin` whose builtin is on the no-throw allowlist.
///
/// Otherwise `Unknown` (fail-closed).
///
/// `resolved_callee` is `None` on the intraprocedural floor and for opaque/builtin
/// targets, so case 2 only fires when the precise module path resolved a body.
fn no_throw_for(op: &TirOp, resolved_callee: Option<&TirFunction>) -> FactValue {
    // (1) The op-kind registry is the single source of truth for may_throw. All
    // three call opcodes have may_throw = true today, but reading the registry
    // (never hardcoding) means a future statically-no-throw call opcode is picked
    // up for free — and keeps the discovery-vs-authority rule (doc 46 §1).
    if !crate::tir::op_kinds_generated::opcode_may_throw_table(op.opcode) {
        return FactValue::Proven;
    }
    // (2) A resolved callee with no handler region.
    if let Some(callee) = resolved_callee
        && !callee.has_exception_handlers()
    {
        return FactValue::Proven;
    }
    // (3) A no-throw-allowlisted builtin.
    if matches!(
        opcode_call_role_table(op.opcode),
        CallOpcodeRole::RuntimeBuiltin
    ) && let Some(name) = builtin_name(op)
        && builtin_is_no_throw(name)
    {
        return FactValue::Proven;
    }
    FactValue::Unknown
}
