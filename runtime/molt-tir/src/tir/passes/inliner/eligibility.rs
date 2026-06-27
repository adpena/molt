use std::collections::HashSet;

use crate::tir::blocks::Terminator;
use crate::tir::call_facts::{InlineEligibility, InlineWhyNot};
use crate::tir::call_graph::CallGraph;
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::op_kinds_generated::opcode_is_state_machine_table;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::values::ValueId;

use super::super::ip_summary::ModuleSummaries;

/// inlining this arc (and likely permanently — these are never simple leaves).
fn is_generator_or_async_op(opcode: OpCode) -> bool {
    opcode_is_state_machine_table(opcode)
}

/// Whether `callee` may be inlined under phases a + b.
///
/// Conservative-correct exclusions (any one disqualifies):
/// * **recursive** — a member of the call graph's recursive set (a recursion
///   cycle, a self-edge, or a function with an opaque call). Inlining a recursive
///   callee is unbounded.
/// * **over budget** — `op_count` exceeds the cost model's
///   [`inline_budget`](crate::tir::target_info::TargetInfo::inline_budget) for
///   this callee. The op count is the same metric the
///   [`ModuleSummaries`](super::ip_summary::ModuleSummaries) records.
/// * **generator / async** — the body contains a state-machine opcode
///   ([`is_generator_or_async_op`]).
/// * **exception HANDLER region** — [`TirFunction::has_exception_handlers`]
///   (`try`/`except` or generator/async state regions). Observation-only callees
///   (`CheckException` with no handler) are NOT excluded — they inline correctly
///   via exception-label remapping + exit routing through the caller's post-call
///   `CheckException`.
/// * **entry block has predecessors** — the splice binds parameters *directly*
///   to the call arguments and clones the callee entry as an argument-less
///   block. That is only SSA-valid when no branch targets the entry (i.e. the
///   entry has zero predecessors — the normal case, since the SSA lift puts
///   loop headers in separate blocks). A callee whose entry block is itself a
///   branch target would need its entry args preserved, which this arc's
///   direct-binding splice does not model, so it is refused (never miscompiled).
/// * **closure** — the callee's first param is the implicit captured-environment
///   param ([`is_closure`] / [`crate::MOLT_CLOSURE_PARAM_NAME`]). The
///   direct param->operand splice would bind that env-param to the `Call`'s
///   LEADING FUNCTION-VALUE operand (operands are `[callee_value, args...]`)
///   instead of the captured environment, miscompiling `__molt_closure__[i]`
///   into a subscript of a function. The arity guard cannot catch this (the
///   closure's extra param re-balances the operand count). Threading the real
///   env is a separate perf arc; refusing is conservative-correct.
pub fn is_inlineable(
    callee: &TirFunction,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
) -> bool {
    // The bool is exactly the eligibility verdict: `is_inlineable` is the
    // single-point reduction of [`classify_inline_eligibility`] (which carries the
    // typed why-not reason the CallFacts side-table records). Reducing here — not
    // duplicating the gates — means the inliner and the CallFacts table can never
    // disagree (doc 47 §7, single source of truth).
    classify_inline_eligibility(callee, call_graph, summaries, tti).is_eligible()
}

/// Whether `callee` may be inlined, and if not, **why** — the typed
/// [`InlineEligibility`] the [`CallFacts`](crate::tir::call_facts) side-table
/// records on each static-direct call site. The single source of truth from which
/// [`is_inlineable`]'s bool is derived (`is_inlineable ==
/// classify_inline_eligibility(...).is_eligible()`).
///
/// Gate-evaluation order (the first failing gate is the reported reason, so the
/// reason is deterministic): the [`inline_safety_gate`] correctness gates
/// (recursion → handlers → generator → entry-predecessor → closure) first, then
/// the cost-model op-count budget ([`InlineWhyNot::OverBudget`]). This matches the
/// short-circuit order of the prior `is_inline_safe && within_budget` predicate
/// exactly, so the bool is byte-identical at every call site.
pub fn classify_inline_eligibility(
    callee: &TirFunction,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
) -> InlineEligibility {
    if let Some(reason) = inline_safety_gate(callee, call_graph) {
        return InlineEligibility::WhyNot(reason);
    }
    if callee_op_count(callee, summaries) > tti.inline_budget(&callee.name) {
        return InlineEligibility::WhyNot(InlineWhyNot::OverBudget);
    }
    InlineEligibility::Eligible
}

/// The callee's op count — the same metric [`ModuleSummaries`] records, with a
/// direct fallback for callees without a summary.
fn callee_op_count(callee: &TirFunction, summaries: &ModuleSummaries) -> usize {
    summaries
        .get(&callee.name)
        .map(|s| s.op_count)
        .unwrap_or_else(|| callee.blocks.values().map(|b| b.ops.len()).sum())
}

/// The first failing **correctness** (safety) gate for inlining `callee`, or
/// `None` if every safety gate passes. This is the single source of truth for the
/// safety verdict, shared by [`is_inline_safe`] (the bool the split-field driver
/// uses) and [`classify_inline_eligibility`] (the typed reason). It deliberately
/// EXCLUDES the cost-model budget — that is [`InlineWhyNot::OverBudget`], applied
/// only by [`classify_inline_eligibility`].
///
/// Gate order is the prior `is_inline_safe` order verbatim:
/// 1. **recursive** — a member of the call graph's recursive set (cycle, self-edge,
///    or opaque-call function). Inlining is unbounded.
/// 2. **exception HANDLER region** — [`TirFunction::has_exception_handlers`]
///    (`try`/`except` or generator/async state regions); the splice does not remap
///    handler labels. Observation-only callees (`CheckException`, no handler) are
///    NOT excluded.
/// 3. **generator / async** — a state-machine opcode ([`is_generator_or_async_op`]).
/// 4. **entry block has predecessors** — the direct param→argument binding splice
///    clones the entry as an argument-less block, valid only when no branch targets
///    the entry.
/// 5. **closure** — the first param is the implicit captured-env param
///    ([`is_closure`]); the direct param→operand splice would miscompile it.
fn inline_safety_gate(callee: &TirFunction, call_graph: &CallGraph) -> Option<InlineWhyNot> {
    if call_graph.recursive_set().contains(&callee.name) {
        return Some(InlineWhyNot::Recursive);
    }
    if callee.has_exception_handlers() {
        return Some(InlineWhyNot::HasHandlers);
    }
    if callee
        .blocks
        .values()
        .any(|b| b.ops.iter().any(|op| is_generator_or_async_op(op.opcode)))
    {
        return Some(InlineWhyNot::Generator);
    }
    if entry_block_has_predecessor(callee) {
        return Some(InlineWhyNot::EntryHasPredecessor);
    }
    if is_closure(callee) {
        return Some(InlineWhyNot::Closure);
    }
    None
}

/// Whether `callee` is SAFE to inline — every correctness gate of
/// [`is_inlineable`] EXCEPT the cost-model op-count budget. Used by the driver to
/// admit a callee that is over-budget but whose inlining UNLOCKS a structural
/// optimization the budget alone cannot see (the split-field deforestation: a
/// caller passes a non-escaping `string_split_field` result into `callee`, so
/// inlining turns the callee's `len(field)`/`ord(field[i])` consumers into
/// bounds-once reads that never materialize the field — see
/// [`split_field_enabled_callees`]). Admitting an over-budget callee here is
/// sound for exactly the same reason every other inline is: the splice is
/// SSA/refcount/loop-metadata preserving regardless of size.
///
/// Delegates to [`inline_safety_gate`] (the single source of truth for the safety
/// verdict) so it can never drift from the typed reason
/// [`classify_inline_eligibility`] reports.
pub(super) fn is_inline_safe(callee: &TirFunction, call_graph: &CallGraph) -> bool {
    inline_safety_gate(callee, call_graph).is_none()
}

/// True if `op` is the `string_split_field` field-access (a `Copy`-passthrough
/// carrying `_original_kind = "string_split_field"`). Its single result is the
/// materialized field — the value whose materialization the deforestation
/// eliminates when every consumer is bounds-expressible.
fn is_string_split_field_op(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy
        && matches!(
            op.attrs.get("_original_kind"),
            Some(AttrValue::Str(k)) if k == "string_split_field"
        )
}

/// The set of module-defined callee names that have at least one direct `Call`
/// site (in any caller) receiving a `string_split_field` result as an argument.
///
/// Inlining such a callee unlocks the split-field deforestation: once the
/// callee body is spliced in, its `len(field)` / `ord(field[i])` consumers read
/// the field that the caller produced, and the SimpleIR deforestation pass
/// rewrites them to bounds-once reads (`molt_string_split_field_*`) so the field
/// never materializes. This is the TARGETED enabling signal the baton specifies
/// — strictly narrower (and icache-cheaper) than a global budget bump, which was
/// measured NOT to help and to regress code size. A callee is admitted only if
/// it is ALSO [`is_inline_safe`]; the splice itself is unconditionally sound.
pub(super) fn split_field_enabled_callees(
    module: &TirModule,
    defined: &[String],
) -> HashSet<String> {
    let defined_set: HashSet<&str> = defined.iter().map(String::as_str).collect();
    let mut enabled: HashSet<String> = HashSet::new();
    for caller in &module.functions {
        // ValueIds produced by a `string_split_field` op in THIS caller.
        let mut field_values: HashSet<ValueId> = HashSet::new();
        for block in caller.blocks.values() {
            for op in &block.ops {
                if is_string_split_field_op(op) {
                    field_values.extend(op.results.iter().copied());
                }
            }
        }
        if field_values.is_empty() {
            continue;
        }
        for block in caller.blocks.values() {
            for op in &block.ops {
                if op.opcode != OpCode::Call {
                    continue;
                }
                let Some(AttrValue::Str(callee_name)) = op.attrs.get("s_value") else {
                    continue;
                };
                if !defined_set.contains(callee_name.as_str()) {
                    continue;
                }
                // Operands are `[args...]` for a resolved `Call` (the static
                // target rides `s_value`, not operand 0); any operand that is a
                // split-field result enables the callee.
                if op.operands.iter().any(|v| field_values.contains(v)) {
                    enabled.insert(callee_name.clone());
                }
            }
        }
    }
    enabled
}

/// True if any block's terminator branches to `callee`'s entry block. The direct
/// param→arg binding splice requires the cloned entry to be argument-less and
/// hence predecessor-free in the callee.
fn entry_block_has_predecessor(callee: &TirFunction) -> bool {
    let entry = callee.entry_block;
    callee.blocks.values().any(|b| match &b.terminator {
        Terminator::Branch { target, .. } => *target == entry,
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => *then_block == entry || *else_block == entry,
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            *default == entry || cases.iter().any(|(_, t, _)| *t == entry)
        }
        Terminator::Return { .. } | Terminator::Unreachable => false,
    })
}

/// True if `callee` is a closure — i.e. its first parameter is the implicit
/// captured-environment param the frontend prepends to every closure
/// (`crate::MOLT_CLOSURE_PARAM_NAME` == `"__molt_closure__"`). This is the same
/// predicate the WASM backend uses to recognize a closure and adjust its arity
/// (`wasm.rs`, on `FunctionIR::params`); here it gates the inliner exclusion (see
/// `is_inlineable`). Both reference the one shared const so the marker has a
/// single source of truth.
///
/// `param_names` is populated on the production lift (`lower_from_simple.rs`),
/// aligned 1:1 with the entry block's arguments, and reliably contains the
/// marker for closures. (The `p{idx}` default from `TirFunction::new` is
/// test-only and never collides with the marker.)
pub(super) fn is_closure(callee: &TirFunction) -> bool {
    callee
        .param_names
        .first()
        .is_some_and(|p| p == crate::MOLT_CLOSURE_PARAM_NAME)
}
