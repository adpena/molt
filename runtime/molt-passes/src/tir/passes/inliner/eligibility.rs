use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::ir::ParameterCustody;
use crate::tir::blocks::Terminator;
use crate::tir::call_facts::{InlineEligibility, InlineWhyNot};
use crate::tir::call_graph::CallGraph;
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::op_kinds_generated::{
    opcode_is_inliner_numeric_raw_lane_consumer_table, opcode_is_state_machine_table,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::{FunctionId, FunctionSet};

use super::super::ip_summary::ModuleSummaries;
use super::activation::releases_unowned_parameter;

/// inlining this arc (and likely permanently - these are never simple leaves).
fn is_generator_or_async_op(opcode: OpCode) -> bool {
    opcode_is_state_machine_table(opcode)
}

/// Whether `callee` may be inlined under phases a + b.
///
/// Conservative-correct exclusions (any one disqualifies):
/// * **recursive** - a member of the call graph's recursive set (a recursion
///   cycle, a self-edge, or a function with an opaque call). Inlining a recursive
///   callee is unbounded.
/// * **over budget** - `op_count` exceeds the cost model's
///   [`inline_budget`](crate::tir::target_info::TargetInfo::inline_budget) for
///   this callee. The op count is the same metric the
///   [`ModuleSummaries`](super::ip_summary::ModuleSummaries) records.
/// * **generator / async** - the body contains a state-machine opcode
///   ([`is_generator_or_async_op`]).
/// * **exception HANDLER region** - [`TirFunction::has_exception_handlers`]
///   (`try`/`except` or generator/async state regions). Observation-only callees
///   (`CheckException` with no handler) are NOT excluded - they inline correctly
///   via exception-label remapping + exit routing through the caller's post-call
///   `CheckException`.
/// * **entry block has predecessors** - the splice binds parameters *directly*
///   to the call arguments and clones the callee entry as an argument-less
///   block. That is only SSA-valid when no branch targets the entry (i.e. the
///   entry has zero predecessors - the normal case, since the SSA lift puts
///   loop headers in separate blocks). A callee whose entry block is itself a
///   branch target would need its entry args preserved, which this arc's
///   direct-binding splice does not model, so it is refused (never miscompiled).
/// * **closure** - its environment must come from the original function object,
///   which a final-TIR target can load at a proven guarded site.
/// * **unowned parameter release** - the body releases a `Borrowed` parameter
///   other than by a Python `del` ([`releases_unowned_parameter`]). The
///   activation owns no reference there, so the release would end the
///   caller's. Parameter custody is otherwise no gate: the splice binds each
///   parameter the activation owns through an owned alias and clears it at
///   every exit.
pub fn is_inlineable(
    callee_id: molt_ir::FunctionId,
    callee: &TirFunction,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
) -> bool {
    // The bool is exactly the eligibility verdict: `is_inlineable` is the
    // single-point reduction of [`classify_inline_eligibility`] (which carries the
    // typed why-not reason the CallFacts side-table records). Reducing here - not
    // duplicating the gates - means the inliner and the CallFacts table can never
    // disagree (doc 47 section7, single source of truth).
    classify_inline_eligibility(callee_id, callee, call_graph, summaries, tti).is_eligible()
}

/// Whether `callee` may be inlined, and if not, **why** - the typed
/// [`InlineEligibility`] the [`CallFacts`](crate::tir::call_facts) side-table
/// records on each static-direct call site. The single source of truth from which
/// [`is_inlineable`]'s bool is derived (`is_inlineable ==
/// classify_inline_eligibility(...).is_eligible()`).
///
/// Gate-evaluation order (the first failing gate is the reported reason, so the
/// reason is deterministic): the [`inline_safety_gate`] correctness gates
/// (execution context -> physical partition -> recursion -> handlers ->
/// generator -> entry-predecessor -> closure -> unowned parameter release)
/// first, then
/// the cost-model op-count budget ([`InlineWhyNot::OverBudget`]). This matches the
/// short-circuit order of the prior `is_inline_safe && within_budget` predicate
/// exactly, so the bool is byte-identical at every call site.
pub fn classify_inline_eligibility(
    callee_id: molt_ir::FunctionId,
    callee: &TirFunction,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
) -> InlineEligibility {
    // A closure callee inlines where its environment is the callee's own
    // closure tuple, which a final-TIR target reads from the proven call's
    // function object (design 78 5.10); the splice admits only those sites.
    let closures = tti.lowering.final_lowering == crate::tir::target_info::FinalLowering::FinalTir;
    if let Some(reason) = inline_safety_gate(callee_id, callee, call_graph, closures) {
        return InlineEligibility::WhyNot(reason);
    }
    if callee_op_count(callee_id, callee, summaries) > tti.inline_budget(&callee.name) {
        return InlineEligibility::WhyNot(InlineWhyNot::OverBudget);
    }
    InlineEligibility::Eligible
}

/// The callee's op count - the same metric [`ModuleSummaries`] records, with a
/// direct fallback for callees without a summary.
pub(super) fn callee_op_count(
    callee_id: molt_ir::FunctionId,
    callee: &TirFunction,
    summaries: &ModuleSummaries,
) -> usize {
    summaries
        .get(callee_id)
        .map(|s| s.op_count)
        .unwrap_or_else(|| callee.blocks.values().map(|b| b.ops.len()).sum())
}

/// The first failing **correctness** (safety) gate for inlining `callee`, or
/// `None` if every safety gate passes. This is the single source of truth for the
/// safety verdict, shared by [`is_inline_safe`] (the bool the split-field driver
/// uses) and [`classify_inline_eligibility`] (the typed reason). It deliberately
/// EXCLUDES the cost-model budget - that is [`InlineWhyNot::OverBudget`], applied
/// only by [`classify_inline_eligibility`].
///
/// Gate order begins with Python execution-context and physical partition
/// restrictions, then preserves the existing structural safety order:
/// 1. **recursive** - a member of the call graph's recursive set (cycle, self-edge,
///    or opaque-call function). Inlining is unbounded.
/// 2. **exception HANDLER region** - [`TirFunction::has_exception_handlers`]
///    (`try`/`except` or generator/async state regions); the splice does not remap
///    handler labels. Observation-only callees (`CheckException`, no handler) are
///    NOT excluded.
/// 3. **generator / async** - a state-machine opcode ([`is_generator_or_async_op`]).
/// 4. **entry block has predecessors** - the direct param->argument binding splice
///    clones the entry as an argument-less block, valid only when no branch targets
///    the entry.
/// 5. **closure** - a target must lower the live captured environment.
/// 6. **unowned parameter release** - an explicit release, other than `del`,
///    of a `Borrowed` parameter ([`releases_unowned_parameter`]); the
///    activation owns no reference to release there.
fn inline_safety_gate(
    callee_id: molt_ir::FunctionId,
    callee: &TirFunction,
    call_graph: &CallGraph,
    closures_at_proven_sites: bool,
) -> Option<InlineWhyNot> {
    if callee.execution_context != molt_ir::ExecutionContextPolicy::None {
        return Some(InlineWhyNot::ExecutionContext);
    }
    if callee.is_codegen_partition() {
        return Some(InlineWhyNot::CodegenPartition);
    }
    if call_graph.recursive_set().contains(callee_id) {
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
    if callee.has_closure_parameter() && !closures_at_proven_sites {
        return Some(InlineWhyNot::Closure);
    }
    if releases_unowned_parameter(callee, |position| {
        callee.parameter_custody(position) != ParameterCustody::Transferred
    }) {
        return Some(InlineWhyNot::UnownedParameterRelease);
    }
    None
}

/// Whether `callee` is SAFE to inline - every correctness gate of
/// [`is_inlineable`] EXCEPT the cost-model op-count budget. Used by the driver to
/// admit a callee that is over-budget but whose inlining UNLOCKS a structural
/// optimization the budget alone cannot see (the split-field deforestation: a
/// caller passes a non-escaping `string_split_field` result into `callee`, so
/// inlining turns the callee's `len(field)`/`ord(field[i])` consumers into
/// bounds-once reads that never materialize the field - see
/// [`split_field_enabled_callees`]). Admitting an over-budget callee here is
/// sound for exactly the same reason every other inline is: the splice is
/// SSA/refcount/loop-metadata preserving regardless of size.
///
/// Delegates to [`inline_safety_gate`] (the single source of truth for the safety
/// verdict) so it can never drift from the typed reason
/// [`classify_inline_eligibility`] reports.
pub(super) fn is_inline_safe(
    callee_id: molt_ir::FunctionId,
    callee: &TirFunction,
    call_graph: &CallGraph,
) -> bool {
    inline_safety_gate(callee_id, callee, call_graph, false).is_none()
}

/// True if `op` is the `string_split_field` field-access (a `Copy`-passthrough
/// carrying `origin = "string_split_field"`). Its single result is the
/// materialized field - the value whose materialization the deforestation
/// eliminates when every consumer is bounds-expressible.
fn is_string_split_field_op(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy
        && matches!(
            op.origin,
            Some(k) if k == molt_ir::OpKind::StringSplitField
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
/// - strictly narrower (and icache-cheaper) than a global budget bump, which was
///   measured NOT to help and to regress code size. A callee is admitted only if
///   it is ALSO [`is_inline_safe`]; the splice itself is unconditionally sound.
pub(super) fn split_field_enabled_callees(
    module: &TirModule,
    defined: &[Option<usize>],
) -> FunctionSet {
    let has_body = |id: FunctionId| defined.get(id.index()).copied().flatten().is_some();
    let mut enabled = FunctionSet::with_capacity(defined.len());
    for caller in &module.functions {
        // ValueIds produced by a `string_split_field` op in THIS caller.
        let mut field_values: HashSet<ValueId> = HashSet::default();
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
                let Some(callee) = op.callee.filter(|callee| has_body(*callee)) else {
                    continue;
                };
                // Operands are `[args...]` for a resolved `Call` (the static
                // target is its typed callee, not operand 0); any operand that
                // is a split-field result enables the callee.
                if op.operands.iter().any(|v| field_values.contains(v)) {
                    enabled.insert(callee);
                }
            }
        }
    }
    enabled
}

/// True when the callee return type can carry a raw numeric lane across the
/// direct-call boundary.
fn is_numeric_raw_lane_return_type(ty: &TirType) -> bool {
    match ty {
        TirType::I64 | TirType::F64 | TirType::Bool => true,
        TirType::Union(members) => {
            let mut saw_numeric = false;
            let mut saw_none = false;
            for member in members {
                match member {
                    TirType::None => saw_none = true,
                    TirType::I64 | TirType::F64 | TirType::Bool if !saw_numeric => {
                        saw_numeric = true;
                    }
                    _ => return false,
                }
            }
            saw_numeric && saw_none
        }
        _ => false,
    }
}

struct NumericRawLaneCandidate<'a> {
    caller: &'a str,
    callee: FunctionId,
    result: ValueId,
    return_type: &'a TirType,
    consumer: OpCode,
}

fn numeric_raw_lane_candidates<'a>(
    module: &'a TirModule,
    defined: &[Option<usize>],
) -> Vec<NumericRawLaneCandidate<'a>> {
    let return_types: HashMap<FunctionId, &TirType> = module
        .ids
        .iter()
        .filter(|id| defined.get(id.index()).copied().flatten().is_some())
        .zip(&module.functions)
        .map(|(id, f)| (*id, &f.return_type))
        .collect();
    if return_types.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    for caller in &module.functions {
        let mut call_result_to_callee: HashMap<ValueId, FunctionId> = HashMap::default();
        for block in caller.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::Call {
                    let Some(callee) = op.callee.filter(|c| return_types.contains_key(c)) else {
                        continue;
                    };
                    if let Some(result) = op.results.first().copied() {
                        call_result_to_callee.insert(result, callee);
                    }
                    continue;
                }
                if !opcode_is_inliner_numeric_raw_lane_consumer_table(op.opcode) {
                    continue;
                }
                for operand in &op.operands {
                    if let Some(&callee) = call_result_to_callee.get(operand) {
                        let return_type = return_types
                            .get(&callee)
                            .expect("candidate callee came from return_types");
                        candidates.push(NumericRawLaneCandidate {
                            caller: caller.name.as_str(),
                            callee,
                            result: *operand,
                            return_type,
                            consumer: op.opcode,
                        });
                    }
                }
            }
        }
    }
    candidates
}

pub(super) fn numeric_raw_lane_candidate_diagnostics(
    module: &TirModule,
    defined: &[Option<usize>],
) -> Vec<String> {
    let mut lines: Vec<String> = numeric_raw_lane_candidates(module, defined)
        .into_iter()
        .map(|candidate| {
            format!(
                "{}:{} result={:?} return={:?} numeric_return={} consumer={:?}",
                candidate.caller,
                module.table.symbol(candidate.callee),
                candidate.result,
                candidate.return_type,
                is_numeric_raw_lane_return_type(candidate.return_type),
                candidate.consumer,
            )
        })
        .collect();
    lines.sort();
    lines
}

/// The set of module-defined callees whose direct-call boundary blocks numeric
/// raw-lane optimization in a caller.
///
/// This is intentionally shaped like [`split_field_enabled_callees`]: it is not a
/// global inline-budget bump. A callee is only enabled when its structural return
/// type is a scalar numeric carrier and one of its direct-call results feeds a
/// numeric op. The driver still applies the full inliner safety gate and caps
/// this exception at the hot-inline budget.
pub(super) fn numeric_raw_lane_enabled_callees(
    module: &TirModule,
    defined: &[Option<usize>],
) -> FunctionSet {
    let mut enabled = FunctionSet::with_capacity(defined.len());
    for candidate in numeric_raw_lane_candidates(module, defined) {
        if is_numeric_raw_lane_return_type(candidate.return_type) {
            enabled.insert(candidate.callee);
        }
    }
    enabled
}

/// True if any block's terminator branches to `callee`'s entry block. The direct
/// param->arg binding splice requires the cloned entry to be argument-less and
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
