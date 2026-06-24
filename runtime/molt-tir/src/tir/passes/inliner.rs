//! TIR **function inliner** — the Tier-2 engine keystone (E1, phases a + b).
//!
//! This is a *module* transform (it splices one function's body into another),
//! not a per-function [`TirPass`](crate::tir::pass_manager). It runs inside
//! [`run_module_pipeline`](crate::tir::module_phase::run_module_pipeline) after
//! the call graph + summaries are built, walks the call graph **bottom-up over
//! the SCC condensation** (every callee is finalized before its callers), and at
//! each statically-resolved, in-budget, exception-free, non-recursive,
//! non-generator call site replaces the `Call` op with a fresh-id clone of the
//! callee's body. After a function has had one or more callees inlined, the
//! per-function S1 [`run_pipeline`](crate::tir::passes::run_pipeline) re-runs on
//! the merged function so the inlined code is optimized *jointly* with the
//! caller (the entire point of inlining — constant-folding the callee's return
//! through the caller's uses, eliminating the call boundary).
//!
//! ## What this arc (phases a + b) does and does NOT do
//!
//! * **(a) clone + remap primitives** — [`clone_function_body_with_fresh_ids`]
//!   produces a disjoint-SSA copy of a callee body inside the caller, with every
//!   `ValueId` / `BlockId` / terminator target / block argument remapped through
//!   the caller's `fresh_value` / `fresh_block` counters. The callee's parameter
//!   values bind *directly* to the call's argument values (no copy ops), so the
//!   cloned entry block carries no arguments. All loop metadata
//!   (`label_id_map` + `loop_roles` + `loop_pairs` + `loop_break_kinds` +
//!   `loop_cond_blocks`) transfers with remapped keys.
//! * **(b) simple splice + module wiring** — [`splice_call_site`] splits the
//!   caller block at the `Call`, branches the first half into the cloned entry,
//!   rewrites each callee `Return` into a branch to the continuation block (which
//!   binds the returned value to the original call-result `ValueId`), and deletes
//!   the `Call`. [`run_inliner`] drives this across the module.
//!
//! Phase c (this arc) extends inlining to **observation-only** callees:
//! functions that carry `CheckException` propagation ops but no real exception
//! HANDLER region (no `try`/`except` `TryStart`/`TryEnd`, no generator/async
//! `StateBlock`). Every callee exit — the normal `Return` AND the exception-exit
//! `Return` (the `ret_void` reached only via `CheckException` edges) — is routed
//! to the continuation block `B_cont`, whose first op is the caller's own
//! post-call `CheckException`; that re-observes the pending flag and routes to
//! the caller's handler exactly as the un-inlined call/return/check sequence did.
//! The clone remaps the callee's per-function exception labels to fresh caller
//! ids (no namespace collision) and pads a void exception-exit's branch into the
//! value-carrying continuation with a representation-matched dead placeholder.
//!
//! Phases d (cost / multi-site / fixed-point) and e (retire the SimpleIR inliner)
//! are SEPARATE later arcs. [`is_inlineable`] still conservatively refuses any
//! callee with a true exception HANDLER region ([`TirFunction::has_exception_handlers`]),
//! any recursive-SCC member, any callee over the cost-model op budget, and any
//! callee containing a generator/async op. Refusing handler-bearing callees is
//! *conservative-correct*, not interim: it never miscompiles, it only forgoes an
//! optimization a later handler-aware arc unlocks.
//!
//! ## The three correctness invariants (each a miscompile if violated)
//!
//! 1. **SSA** — the splice is structurally SSA-preserving: the continuation
//!    block is reachable *only* through the cloned callee's exits, every one of
//!    which is dominated by the cloned entry, and the call-result value is
//!    redefined as the continuation block's single argument. Every splice is
//!    followed by a `verify_function` assertion (in tests) and the
//!    [`run_pipeline`](crate::tir::passes::run_pipeline) re-run (which itself
//!    verifies). A splice that produced invalid SSA *panics*; it never silently
//!    corrupts.
//! 2. **REFCOUNT** — the calling convention is **+0 borrowed** parameters /
//!    **+1 owned** return. The splice adds and removes *zero* `IncRef`/`DecRef`
//!    ops, so the callee body's reference-count balance is preserved verbatim.
//!    The one caller-side hazard: a caller that does `IncRef(arg)` immediately
//!    before the `Call` (handing the callee an owned, not borrowed, argument)
//!    would, post-inline, leak that extra reference because the callee body
//!    consumes a *borrowed* parameter. [`splice_call_site`] therefore refuses any
//!    site with an `IncRef` of one of the call's argument values in the ≤2 ops
//!    immediately preceding the `Call` (the [`call_site_has_arg_incref`] guard).
//! 3. **LOOP METADATA** — LICM / BCE / the structured-loop back-conversion read
//!    `loop_roles` *and* `loop_pairs` *and* `loop_break_kinds` *and*
//!    `loop_cond_blocks`. Transferring only `loop_roles` (the obvious one) would
//!    leave the merged loop half-described and mis-optimized. The clone transfers
//!    **all four** maps (plus `label_id_map`) with every key remapped to the
//!    fresh block ids.

use std::collections::{HashMap, HashSet};

use super::super::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use super::super::call_facts::{InlineEligibility, InlineWhyNot};
use super::super::call_graph::CallGraph;
use super::super::dominators::{CfgEdgePolicy, reachable_blocks_with};
use super::super::function::{TirFunction, TirModule};
use super::super::op_kinds_generated::{
    opcode_has_exception_label_attr_table, opcode_is_state_machine_table,
};
use super::super::ops::{AttrDict, AttrValue, OpCode, TirOp, dead_placeholder_const_for_type};
use super::super::target_info::TargetInfo;
use super::super::types::TirType;
use super::super::values::{TirValue, ValueId};
use super::ip_summary::ModuleSummaries;

/// Statistics from one [`run_inliner`] invocation over a module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlinerStats {
    /// Number of call sites successfully inlined (a `Call` replaced by the
    /// callee body).
    pub sites_inlined: usize,
    /// Number of caller functions that had at least one site inlined (and were
    /// therefore re-optimized by the per-function pipeline).
    pub functions_changed: usize,
    /// Names of the caller functions that had at least one site inlined (and so
    /// whose body now differs from its pre-inline form). Production codegen
    /// back-converts ONLY these functions' TIR to SimpleIR, leaving every
    /// unchanged function byte-identical (no second TIR roundtrip).
    pub changed_functions: Vec<String>,
}

/// The product of cloning a callee body into a caller: the block id the call's
/// predecessor half must branch into (the cloned callee entry), and the set of
/// fresh block ids that make up the cloned body (so the splicer can locate the
/// cloned `Return`-bearing blocks to rewrite into continuation branches).
struct ClonedCallee {
    /// The fresh `BlockId` of the cloned callee's entry block. The caller's
    /// pre-call half branches here. This block has **no arguments** — the
    /// callee's parameters were bound directly to the call arguments.
    entry: BlockId,
    /// Every fresh block id introduced by the clone, in deterministic order.
    cloned_blocks: Vec<BlockId>,
    /// The callee `BlockId` → cloned `BlockId` map. The splicer uses it to carry
    /// the callee-side classification of each `Return` block (normal-return vs
    /// exception-exit, computed on the callee's terminator-only CFG) onto the
    /// cloned blocks when rewriting `Return`s into continuation branches.
    block_map: HashMap<BlockId, BlockId>,
}

/// Read an op's `s_value` string attribute, if present.
fn s_value(op: &TirOp) -> Option<&str> {
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Clone an op's attribute dict while dropping the SimpleIR value-name
/// annotations (`_simple_out` and `_simple_result_N`).
///
/// Cloning a callee body remaps every `ValueId`/`BlockId` to a fresh id, but
/// these annotations are *function-local name strings* (a Python local like `x`
/// or `i`) with no id to remap — a verbatim copy carries the callee's names into
/// the caller. If a callee name collides with a caller value of a different
/// container kind, the name-keyed container-dispatch plan
/// (`LlvmReprFacts::container_kind`, the only `_simple_out`-keyed reader on the
/// merged TIR) would resolve the inlined value to the *caller's* kind — a wrong
/// `molt_len_*` selection, i.e. a miscompile. It would likewise alias two values
/// onto one SimpleIR slot in the native TIR→SimpleIR lowering.
///
/// Dropping the names lets each inlined value fall to its unique canonical
/// (`ValueId`-derived) name, so it is classified by the authoritative
/// `ValueId`-keyed `TirType` instead. Freshly-built inlined containers keep their
/// concrete `TirType`, so the correct kind is preserved for the common case; only
/// a `DynBox`-typed inlined container loses `len` specialization (sound — generic
/// dispatch). The soundness-critical integer-carrier `repr_by_value` is already
/// `ValueId`-keyed and is unaffected either way.
fn clone_attrs_without_simple_names(attrs: &AttrDict) -> AttrDict {
    attrs
        .iter()
        .filter(|(k, _)| k.as_str() != "_simple_out" && !k.starts_with("_simple_result_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The fixed runtime-intrinsic `s_value` symbols that lift to `OpCode::Call` but
/// are runtime-helper calls (gpu_*), never user-defined functions. They are not
/// inlinable call sites (there is no module-defined body to inline). Mirrors the
/// call-graph's `is_gpu_runtime_symbol`.
fn is_gpu_runtime_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "molt_gpu_thread_id"
            | "molt_gpu_block_id"
            | "molt_gpu_block_dim"
            | "molt_gpu_grid_dim"
            | "molt_gpu_barrier"
    )
}

/// The generator / async / coroutine opcodes. A callee containing any of these
/// is a state-machine function whose body cannot be linearly spliced into a
/// caller without reconstructing the suspension machinery; it is excluded from
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
fn is_inline_safe(callee: &TirFunction, call_graph: &CallGraph) -> bool {
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
fn split_field_enabled_callees(module: &TirModule, defined: &[String]) -> HashSet<String> {
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
fn is_closure(callee: &TirFunction) -> bool {
    callee
        .param_names
        .first()
        .is_some_and(|p| p == crate::MOLT_CLOSURE_PARAM_NAME)
}

/// One statically-resolvable, inlinable call site inside a caller block.
struct CallSite {
    /// The caller block containing the `Call`.
    block: BlockId,
    /// The op index of the `Call` within that block's `ops`.
    op_index: usize,
    /// The callee name (a module-defined function).
    callee: String,
}

/// Collect every statically-direct `Call` op in `caller` whose target is a
/// module-defined function (resolved via `s_value`), in deterministic order
/// (blocks sorted by id, ops in index order). Opaque calls, method dispatch,
/// builtin calls, gpu intrinsics, and copy-fallback calls are NOT collected —
/// only a first-class `Call` with an `s_value` naming a `defined` function.
fn collect_call_sites(caller: &TirFunction, defined: &[String]) -> Vec<CallSite> {
    let defined_set: std::collections::BTreeSet<&str> =
        defined.iter().map(String::as_str).collect();
    let mut sites = Vec::new();
    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    for bid in block_ids {
        let block = &caller.blocks[&bid];
        for (op_index, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::Call {
                continue;
            }
            let Some(name) = s_value(op) else { continue };
            if is_gpu_runtime_symbol(name) {
                continue;
            }
            if !defined_set.contains(name) {
                continue;
            }
            sites.push(CallSite {
                block: bid,
                op_index,
                callee: name.to_string(),
            });
        }
    }
    sites
}

/// REFCOUNT guard: returns true if any of the call's argument values is the
/// result of an `IncRef` in the ≤2 ops immediately before the `Call`. Such a
/// site hands the callee an *owned* argument (the `IncRef` balances a `DecRef`
/// the callee would issue under a +1 convention, or the caller is materializing
/// an owned temporary). Inlining a +0-borrowed-parameter body there would leak
/// the extra reference, so the site is refused.
///
/// `IncRef`'s reference target is its operand (the value being retained). We
/// scan the two preceding ops for an `IncRef` whose operand is one of the call's
/// argument operands.
fn call_site_has_arg_incref(block: &TirBlock, call_op_index: usize, call_args: &[ValueId]) -> bool {
    if call_args.is_empty() {
        return false;
    }
    let arg_set: std::collections::HashSet<ValueId> = call_args.iter().copied().collect();
    let lo = call_op_index.saturating_sub(2);
    for op in &block.ops[lo..call_op_index] {
        if op.opcode == OpCode::IncRef && op.operands.iter().any(|v| arg_set.contains(v)) {
            return true;
        }
    }
    false
}

/// Clone `callee`'s body into `caller`, minting fresh `ValueId`/`BlockId`s for
/// everything except the callee parameters, which are bound directly to
/// `arg_values` (the call's argument values, already valid in `caller`).
///
/// Returns the cloned-entry block id (which the splice's pre-call half branches
/// into) and the list of fresh block ids. The caller is responsible for actually
/// inserting the resulting blocks (they are inserted into `caller.blocks` here)
/// and for wiring the pre/cont split + rewriting cloned `Return`s — that is
/// [`splice_call_site`]'s job.
///
/// Invariants established here:
/// * The cloned entry block has **no arguments** (params bind to `arg_values`).
/// * Every callee value not a parameter gets a fresh id; uses are remapped.
/// * Cloned `Return` terminators are left *as `Return`* — the splicer rewrites
///   them into branches to the continuation (it owns the continuation id).
/// * `value_types` for cloned values transfer (remapped keys) so type facts
///   survive into the merged function.
/// * All loop metadata (`label_id_map` + `loop_roles` + `loop_pairs` +
///   `loop_break_kinds` + `loop_cond_blocks`) transfer with remapped keys.
/// * **Exception labels** are remapped to fresh caller ids. SimpleIR label ids
///   are per-function (`next_label()` resets per function), so the callee's
///   labels routinely collide numerically with the caller's. Both the cloned
///   `CheckException`/`TryStart`/`TryEnd` `"value"` attrs (transfer labels for
///   `CheckException`/`TryStart`, pairing metadata for `TryEnd`) AND the cloned
///   blocks' `label_id_map` entries are remapped through one
///   [`build_label_remap`] table so the merged function's exception-transfer
///   edges resolve to the cloned exit block (not a colliding caller block) and
///   `lower_to_simple` emits no duplicate `label N` ops.
fn clone_function_body_with_fresh_ids(
    callee: &TirFunction,
    caller: &mut TirFunction,
    arg_values: &[ValueId],
) -> ClonedCallee {
    // Fresh exception/label remap for this clone. Allocated ABOVE the caller's
    // current max label so it cannot collide with any caller label (including the
    // fresh labels of callees already inlined into this caller — `caller` is
    // re-scanned each clone, and each clone's fresh labels were inserted into
    // `caller.label_id_map` by `transfer_loop_metadata`).
    let label_remap = build_label_remap(callee, caller);

    // Value remap: callee ValueId -> caller ValueId. Pre-seed the parameters to
    // bind directly to the call's argument values.
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();
    let entry = &callee.blocks[&callee.entry_block];
    debug_assert_eq!(
        entry.args.len(),
        arg_values.len(),
        "inliner: callee '{}' has {} params but call passed {} args",
        callee.name,
        entry.args.len(),
        arg_values.len()
    );
    for (param, arg) in entry.args.iter().zip(arg_values.iter()) {
        value_map.insert(param.id, *arg);
    }

    // Block remap: callee BlockId -> fresh caller BlockId. Deterministic order
    // (sorted by callee block id) so the fresh-id assignment is reproducible.
    let mut callee_block_ids: Vec<BlockId> = callee.blocks.keys().copied().collect();
    callee_block_ids.sort_by_key(|b| b.0);
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::new();
    for &bid in &callee_block_ids {
        block_map.insert(bid, caller.fresh_block());
    }

    // Mint fresh value ids for every non-parameter callee result and every
    // non-entry block argument, in a deterministic walk (blocks sorted; within a
    // block, args then ops in order).
    let fresh_for = |old: ValueId,
                     value_map: &mut HashMap<ValueId, ValueId>,
                     caller: &mut TirFunction|
     -> ValueId {
        if let Some(&existing) = value_map.get(&old) {
            return existing;
        }
        let fresh = caller.fresh_value();
        value_map.insert(old, fresh);
        fresh
    };

    for &bid in &callee_block_ids {
        let block = &callee.blocks[&bid];
        // Entry-block args are the parameters — already bound to arg_values, so
        // do NOT mint fresh ids for them. Non-entry block args get fresh ids.
        if bid != callee.entry_block {
            for arg in &block.args {
                fresh_for(arg.id, &mut value_map, caller);
            }
        }
        for op in &block.ops {
            for result in &op.results {
                fresh_for(*result, &mut value_map, caller);
            }
        }
    }

    // Helper to remap a single value (must already be in the map — every defined
    // value was assigned above; every used value is either a param, a prior
    // def, or a block arg, all of which are mapped).
    let remap = |v: ValueId, value_map: &HashMap<ValueId, ValueId>| -> ValueId {
        *value_map.get(&v).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' uses value {} with no remap (malformed SSA?)",
                callee.name, v
            )
        })
    };
    let remap_block = |b: BlockId| -> BlockId {
        *block_map.get(&b).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' references block {} with no remap",
                callee.name, b
            )
        })
    };

    // Build the cloned blocks.
    for &bid in &callee_block_ids {
        let src = &callee.blocks[&bid];
        let new_bid = remap_block(bid);

        // Cloned block arguments: empty for the entry (params bound to args),
        // remapped for every other block.
        let new_args: Vec<TirValue> = if bid == callee.entry_block {
            Vec::new()
        } else {
            src.args
                .iter()
                .map(|a| TirValue {
                    id: remap(a.id, &value_map),
                    ty: a.ty.clone(),
                })
                .collect()
        };

        // Cloned ops with operands/results remapped. The SimpleIR value-name
        // annotations are dropped (see `clone_attrs_without_simple_names`): they
        // are function-local name strings with no id to remap, so a verbatim copy
        // would carry the callee's names into the caller and collide with caller
        // values of the same name. Exception ops additionally have their handler
        // `"value"` label remapped (see `remap_exception_label_attr`) so the
        // cloned exception edge resolves to the cloned exit block, not a caller
        // block that happens to share the callee's original (per-function) label.
        let new_ops: Vec<TirOp> = src
            .ops
            .iter()
            .map(|op| {
                let mut attrs = clone_attrs_without_simple_names(&op.attrs);
                remap_exception_label_attr(op.opcode, &mut attrs, &label_remap);
                TirOp {
                    dialect: op.dialect,
                    opcode: op.opcode,
                    operands: op.operands.iter().map(|v| remap(*v, &value_map)).collect(),
                    results: op.results.iter().map(|v| remap(*v, &value_map)).collect(),
                    attrs,
                    source_span: op.source_span,
                }
            })
            .collect();

        // Cloned terminator with targets + value operands remapped. `Return`s
        // stay `Return` (the splicer rewrites them); every other terminator's
        // block targets and value args remap.
        let new_term = clone_terminator(&src.terminator, &value_map, &block_map, callee);

        caller.blocks.insert(
            new_bid,
            TirBlock {
                id: new_bid,
                args: new_args,
                ops: new_ops,
                terminator: new_term,
            },
        );
    }

    // Transfer value_types for every cloned value (remapped key). Skip params
    // (they map to caller arg values that already carry their own types).
    let entry_param_ids: std::collections::HashSet<ValueId> =
        entry.args.iter().map(|a| a.id).collect();
    for (old, ty) in &callee.value_types {
        if entry_param_ids.contains(old) {
            continue;
        }
        if let Some(&new) = value_map.get(old) {
            caller.value_types.entry(new).or_insert_with(|| ty.clone());
        }
    }

    // Transfer loop metadata — ALL FOUR maps plus label_id_map — with remapped
    // keys (and remapped values where the value is itself a block id). Missing
    // any of these mis-describes the merged loops to LICM / BCE / the structured
    // back-conversion. `label_id_map` LABEL VALUES are remapped through
    // `label_remap` (matching the exception-op `"value"` attr remap above) so the
    // cloned blocks carry collision-free labels.
    transfer_loop_metadata(callee, caller, &block_map, &label_remap);

    ClonedCallee {
        entry: remap_block(callee.entry_block),
        cloned_blocks: callee_block_ids.iter().map(|b| remap_block(*b)).collect(),
        block_map,
    }
}

/// Clone a terminator, remapping value operands and block targets. `Return`
/// terminators are cloned verbatim (values remapped) — the splicer rewrites them
/// into branches once it owns the continuation block id.
fn clone_terminator(
    term: &Terminator,
    value_map: &HashMap<ValueId, ValueId>,
    block_map: &HashMap<BlockId, BlockId>,
    callee: &TirFunction,
) -> Terminator {
    let rv = |v: ValueId| -> ValueId {
        *value_map.get(&v).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' terminator uses value {} with no remap",
                callee.name, v
            )
        })
    };
    let rb = |b: BlockId| -> BlockId {
        *block_map.get(&b).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' terminator targets block {} with no remap",
                callee.name, b
            )
        })
    };
    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: rb(*target),
            args: args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: rv(*cond),
            then_block: rb(*then_block),
            then_args: then_args.iter().map(|v| rv(*v)).collect(),
            else_block: rb(*else_block),
            else_args: else_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: rv(*value),
            cases: cases
                .iter()
                .map(|(c, blk, args)| (*c, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(s, blk, args)| (*s, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

/// Transfer `label_id_map` + `loop_roles` + `loop_pairs` + `loop_break_kinds` +
/// `loop_cond_blocks` from the callee into the caller, remapping every block-id
/// key (and any block-id-valued entry) through `block_map`. `label_id_map` LABEL
/// VALUES are additionally remapped through `label_remap` (the same table that
/// rewrote the cloned exception ops' `"value"` attrs), so a cloned block's label
/// matches the cloned exception edge that targets it and cannot collide with a
/// caller label that shared the callee's original per-function label id.
fn transfer_loop_metadata(
    callee: &TirFunction,
    caller: &mut TirFunction,
    block_map: &HashMap<BlockId, BlockId>,
    label_remap: &HashMap<i64, i64>,
) {
    // label_id_map is keyed by BlockId.0 (a raw u32). Remap the key through the
    // block map AND the label value through `label_remap` so the cloned
    // exception/jump targets carry collision-free labels in the merged function.
    for (old_block_u32, label_val) in &callee.label_id_map {
        if let Some(new_bid) = block_map.get(&BlockId(*old_block_u32)) {
            let new_label = label_remap.get(label_val).copied().unwrap_or(*label_val);
            caller.label_id_map.entry(new_bid.0).or_insert(new_label);
        }
    }
    // loop_roles: BlockId -> LoopRole.
    for (old_bid, role) in &callee.loop_roles {
        if let Some(new_bid) = block_map.get(old_bid) {
            caller
                .loop_roles
                .entry(*new_bid)
                .or_insert_with(|| clone_loop_role(role));
        }
    }
    // loop_pairs: header BlockId -> end BlockId (both remap).
    for (old_header, old_end) in &callee.loop_pairs {
        if let (Some(new_header), Some(new_end)) =
            (block_map.get(old_header), block_map.get(old_end))
        {
            caller.loop_pairs.entry(*new_header).or_insert(*new_end);
        }
    }
    // loop_break_kinds: header BlockId -> LoopBreakKind.
    for (old_header, kind) in &callee.loop_break_kinds {
        if let Some(new_header) = block_map.get(old_header) {
            caller
                .loop_break_kinds
                .entry(*new_header)
                .or_insert(clone_loop_break_kind(kind));
        }
    }
    // loop_cond_blocks: header BlockId -> cond BlockId (both remap).
    for (old_header, old_cond) in &callee.loop_cond_blocks {
        if let (Some(new_header), Some(new_cond)) =
            (block_map.get(old_header), block_map.get(old_cond))
        {
            caller
                .loop_cond_blocks
                .entry(*new_header)
                .or_insert(*new_cond);
        }
    }
}

fn clone_loop_role(role: &LoopRole) -> LoopRole {
    match role {
        LoopRole::None => LoopRole::None,
        LoopRole::LoopHeader => LoopRole::LoopHeader,
        LoopRole::LoopEnd => LoopRole::LoopEnd,
    }
}

fn clone_loop_break_kind(kind: &LoopBreakKind) -> LoopBreakKind {
    *kind
}

/// The opcodes whose `"value"` attribute is a SimpleIR **label id** naming an
/// exception/handler target block (read by [`crate::tir::dominators::exception_successors`]
/// and re-emitted by `lower_to_simple`). These are the only in-block ops that
/// reference a block by label rather than by `BlockId`, so they are the only ops
/// whose attrs need label remapping when a body is cloned.
fn is_exception_label_op(opcode: OpCode) -> bool {
    opcode_has_exception_label_attr_table(opcode)
}

/// Read the label id from an exception op's `"value"` attr, if present.
fn exception_label_of(op: &TirOp) -> Option<i64> {
    if !is_exception_label_op(op.opcode) {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

/// The set of SimpleIR label ids `func` uses: the union of every `label_id_map`
/// value and every exception op's `"value"` label. `label_id_map` already covers
/// every label-bearing block, but the exception-op attrs are unioned in so a
/// callee whose exception target is (defensively) missing from `label_id_map`
/// still gets a fresh remap rather than an accidental passthrough collision.
fn function_label_ids(func: &TirFunction) -> std::collections::BTreeSet<i64> {
    let mut labels: std::collections::BTreeSet<i64> = func.label_id_map.values().copied().collect();
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(label) = exception_label_of(op) {
                labels.insert(label);
            }
        }
    }
    labels
}

/// Build the callee→fresh label remap for one clone. Every label the callee uses
/// is reassigned to a fresh id strictly greater than every label currently in the
/// caller, so the cloned body's exception labels cannot collide with the caller's
/// (or with the fresh labels of callees already inlined into this caller — those
/// were inserted into `caller.label_id_map`, so the caller's max grows with each
/// clone). Deterministic: callee labels are processed in ascending order.
fn build_label_remap(callee: &TirFunction, caller: &TirFunction) -> HashMap<i64, i64> {
    let callee_labels = function_label_ids(callee);
    if callee_labels.is_empty() {
        return HashMap::new();
    }
    let caller_max = function_label_ids(caller).iter().copied().max();
    // Start strictly above the caller's max (or at 0 if the caller has no labels).
    let start = caller_max.map(|m| m + 1).unwrap_or(0);
    let mut remap = HashMap::with_capacity(callee_labels.len());
    // Callee labels are processed in ascending order; each gets the next id
    // counting up from `start` (the `start..` range supplies the counter).
    for (label, next) in callee_labels.into_iter().zip(start..) {
        remap.insert(label, next);
    }
    remap
}

/// Rewrite a cloned exception op's `"value"` label attr through `label_remap`.
/// A non-exception op, or an exception label not present in the remap, is left
/// untouched (a missing remap entry can only happen for a label the callee did
/// not actually declare, which `function_label_ids` already folds in, so in
/// practice every cloned exception label is remapped).
fn remap_exception_label_attr(
    opcode: OpCode,
    attrs: &mut AttrDict,
    label_remap: &HashMap<i64, i64>,
) {
    if !is_exception_label_op(opcode) {
        return;
    }
    if let Some(AttrValue::Int(old_label)) = attrs.get("value")
        && let Some(&new_label) = label_remap.get(old_label)
    {
        attrs.insert("value".into(), AttrValue::Int(new_label));
    }
}

/// Splice the call site `(block, op_index)` in `caller`: replace the `Call` to
/// `callee` (an owned snapshot) with the callee's inlined body.
///
/// The callee is passed by reference rather than looked up inside, because the
/// driver holds `&mut caller` borrowed out of the module vector and Rust cannot
/// prove disjointness from a second borrow of the callee through the same
/// vector. The driver clones the callee snapshot (`callee_idx != caller_idx` is
/// guaranteed — self-calls are filtered) and hands it here.
///
/// Returns `true` if the site was inlined, `false` if it was refused (refcount
/// guard, multi-result/arity/shape mismatch — all of which leave the call
/// intact, conservative-correct).
///
/// Mechanics:
/// 1. Read the `Call` op's argument operands and (optional) result value.
/// 2. Refcount guard — refuse a site with a caller-side arg `IncRef` in the ≤2
///    preceding ops.
/// 3. Clone the callee body (params bound to the call args) into `caller`.
/// 4. Split the caller block at the `Call` into `B_pre` (ops `0..op_index`,
///    keeping the original block id) and a fresh continuation `B_cont` (ops
///    `op_index+1..`, taking the original terminator). `B_cont`'s single block
///    argument is the original call-result value id, so every downstream use of
///    the call result is satisfied without rewriting.
/// 5. `B_pre` branches unconditionally into the cloned entry.
/// 6. Each cloned `Return { values }` becomes `Branch { target: B_cont, args:
///    values }` (or `Branch B_cont []` for a void callee with a no-arg `B_cont`).
/// 7. The original `Call` op is gone (it lived between `B_pre` and `B_cont`).
fn splice_call_site(caller: &mut TirFunction, callee: &TirFunction, site: &CallSite) -> bool {
    let block_id = site.block;
    let op_index = site.op_index;

    let (call_args, call_result, multi_result): (Vec<ValueId>, Option<ValueId>, bool) = {
        let block = &caller.blocks[&block_id];
        let op = &block.ops[op_index];
        if op.opcode != OpCode::Call {
            return false;
        }
        (
            op.operands.clone(),
            op.results.first().copied(),
            op.results.len() > 1,
        )
    };
    if multi_result {
        return false;
    }

    // DEFENSE-IN-DEPTH: never splice a closure here. `is_inlineable` already
    // excludes closures (see `is_closure`), so a closure should never reach this
    // splice. But the arity guard below CANNOT distinguish a closure from a
    // legitimate same-arity call — a closure's leading `__molt_closure__` param
    // re-balances against the `Call`'s leading function-value operand, so the
    // guard would pass (false match) and the splice would bind the env-param to
    // the function object, miscompiling `__molt_closure__[i]` into a subscript of
    // a function. Refuse structurally so a future `is_inlineable` change cannot
    // silently re-open the hole. Refusal (not panic) keeps a release build sound;
    // the debug assert flags the invariant violation in tests.
    debug_assert!(
        !is_closure(callee),
        "splice_call_site: closure '{}' reached splice — is_inlineable must \
         exclude closures (the arity guard cannot, its env-param re-balances \
         the operand count)",
        callee.name
    );
    if is_closure(callee) {
        return false;
    }

    // Arity must match (params bind 1:1 to args). A static call whose arg count
    // disagrees with the callee's param count is a shape we will not splice
    // (defensive — the frontend should keep these aligned, but a mismatch must
    // not produce malformed SSA).
    let callee_entry = &callee.blocks[&callee.entry_block];
    if callee_entry.args.len() != call_args.len() {
        return false;
    }

    // REFCOUNT guard (invariant 2).
    if call_site_has_arg_incref(&caller.blocks[&block_id], op_index, &call_args) {
        return false;
    }

    // Classify the callee's `Return` blocks on its **terminator-only** CFG:
    //  * NORMAL return — reachable from entry through terminator edges. Carries
    //    the function's actual return value.
    //  * EXCEPTION EXIT — reachable ONLY via implicit exception edges
    //    (`CheckException` → function-exit). A `ret_void` "propagate the pending
    //    flag" exit; it carries no value.
    // This classification (computed on the callee, before any mutation) drives
    // both the pre-check below and the placeholder padding in the rewrite loop.
    let normal_reachable = reachable_blocks_with(callee, CfgEdgePolicy::TerminatorOnly);

    // Return-arity compatibility pre-check (BEFORE any mutation, so a refusal
    // leaves `caller` byte-identical — no fragile mid-splice rollback). The
    // continuation carries one argument iff the call produces a value. Every
    // NORMAL-return site must then carry a value: a value call demands exactly
    // one returned value from each normal return. A normal return that carries
    // *no* value while the call expects one is a frontend-shape mismatch we
    // refuse rather than fabricate a value for. (An EXCEPTION-EXIT carries no
    // value by construction; it is handled by placeholder padding, not refused —
    // refusing it would re-dormant the inliner on every value-returning
    // observation-only callee, which is the whole point of this arc.)
    let call_wants_value = call_result.is_some();
    if call_wants_value {
        for (bid, block) in &callee.blocks {
            if let Terminator::Return { values } = &block.terminator
                && normal_reachable.contains(bid)
                && values.is_empty()
            {
                return false;
            }
        }
    }

    // Clone the callee body into the caller (params → call args).
    let cloned = clone_function_body_with_fresh_ids(callee, caller, &call_args);

    // The cloned block ids of the callee's EXCEPTION-EXIT blocks (reached only via
    // exception edges). Their cloned `Return`s need placeholder padding when the
    // continuation carries a value.
    let exception_exit_clones: std::collections::HashSet<BlockId> = callee
        .blocks
        .keys()
        .filter(|bid| !normal_reachable.contains(bid))
        .filter_map(|bid| cloned.block_map.get(bid).copied())
        .collect();

    // Split the caller block. Take the original block out, partition its ops.
    let original = caller
        .blocks
        .remove(&block_id)
        .expect("splice: caller block vanished");
    let TirBlock {
        id: _,
        args: pre_args,
        ops: mut all_ops,
        terminator: original_term,
    } = original;

    // Ops after the call become the continuation block's ops.
    let cont_ops = all_ops.split_off(op_index + 1);
    // Remove the `Call` op itself (now the last element of `all_ops`).
    let removed_call_opcode = all_ops.pop().map(|o| o.opcode);
    assert_eq!(
        removed_call_opcode,
        Some(OpCode::Call),
        "splice: expected to remove the Call op at {block_id:?}#{op_index}"
    );
    let pre_ops = all_ops;

    // The continuation block takes a single argument = the original call result
    // value id (when the call produced a value). A void call → no-arg cont.
    let cont_block_id = caller.fresh_block();
    let cont_args: Vec<TirValue> = match call_result {
        Some(result) => {
            let ty = caller
                .value_types
                .get(&result)
                .cloned()
                .or_else(|| callee_return_value_type(callee))
                .unwrap_or(super::super::types::TirType::DynBox);
            vec![TirValue { id: result, ty }]
        }
        None => Vec::new(),
    };

    // Rewrite each cloned `Return { values }` into a branch to the continuation.
    //  * A NORMAL return (value call): branch with the returned value — the
    //    pre-check guarantees it carries one.
    //  * An EXCEPTION-EXIT return (`ret_void`) into a value-carrying continuation:
    //    synthesize a representation-matched DEAD placeholder for the missing
    //    continuation arg. The value is provably dead — `B_cont`'s first op is the
    //    caller's post-call `CheckException`, which re-observes the pending flag
    //    and reroutes before the call result is ever used — so the placeholder is
    //    never read. `verify_block_args` checks only arity; the typed placeholder
    //    keeps the continuation phi's representation clean for codegen.
    //  * A void call (cont_arity 0): branch with no args (any returned value, on
    //    the normal or exception path, is discarded — the call discarded it too).
    let cont_arity = cont_args.len();
    let cont_ty: Option<TirType> = cont_args.first().map(|a| a.ty.clone());
    debug_assert!(
        cont_arity <= 1,
        "continuation arity is 0 (void) or 1 (value)"
    );
    for &cloned_bid in &cloned.cloned_blocks {
        let return_values: Option<Vec<ValueId>> = match &caller.blocks[&cloned_bid].terminator {
            Terminator::Return { values } => Some(values.clone()),
            _ => None,
        };
        let Some(values) = return_values else {
            continue;
        };

        let branch_args: Vec<ValueId> = match (cont_arity, values.first()) {
            (0, _) => Vec::new(),
            (1, Some(&v)) => vec![v],
            (1, None) => {
                // Void return into a value-carrying continuation. The pre-check
                // refused any NORMAL return that carries no value, so this is
                // exclusively an exception-exit.
                debug_assert!(
                    exception_exit_clones.contains(&cloned_bid),
                    "void return survived the pre-check in a non-exception-exit block"
                );
                let ty = cont_ty.clone().unwrap_or(TirType::DynBox);
                let placeholder = caller.fresh_value();
                caller.value_types.entry(placeholder).or_insert(ty.clone());
                let const_op = dead_placeholder_const_for_type(&ty, placeholder);
                caller
                    .blocks
                    .get_mut(&cloned_bid)
                    .expect("cloned block missing")
                    .ops
                    .push(const_op);
                vec![placeholder]
            }
            _ => unreachable!("continuation arity is 0 or 1 (debug-asserted)"),
        };
        caller
            .blocks
            .get_mut(&cloned_bid)
            .expect("cloned block missing")
            .terminator = Terminator::Branch {
            target: cont_block_id,
            args: branch_args,
        };
    }

    // Insert B_pre (original id, ops 0..call, branch into cloned entry).
    caller.blocks.insert(
        block_id,
        TirBlock {
            id: block_id,
            args: pre_args,
            ops: pre_ops,
            terminator: Terminator::Branch {
                target: cloned.entry,
                args: Vec::new(),
            },
        },
    );

    // Insert B_cont (continuation: the cont arg + the post-call ops + original
    // terminator).
    caller.blocks.insert(
        cont_block_id,
        TirBlock {
            id: cont_block_id,
            args: cont_args,
            ops: cont_ops,
            terminator: original_term,
        },
    );

    true
}

/// The type the callee returns, derived from its `Return` terminators'
/// value_types (best-effort, for annotating the continuation block arg).
fn callee_return_value_type(callee: &TirFunction) -> Option<super::super::types::TirType> {
    for block in callee.blocks.values() {
        if let Terminator::Return { values } = &block.terminator
            && let Some(v) = values.first()
            && let Some(ty) = callee.value_types.get(v)
        {
            return Some(ty.clone());
        }
    }
    if callee.return_type != super::super::types::TirType::None {
        return Some(callee.return_type.clone());
    }
    None
}

/// Run the inliner over `module` in **bottom-up SCC order** (callees finalized
/// before callers). After a function has one or more sites inlined, re-run the
/// per-function pipeline on the merged function so the inlined body is optimized
/// jointly with the caller.
///
/// `call_graph` and `summaries` describe the module *before* this pass; the
/// driver ([`run_module_pipeline`](crate::tir::module_phase::run_module_pipeline))
/// rebuilds both afterward.
pub fn run_inliner(
    module: &mut TirModule,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
    non_inlinable: &HashSet<String>,
) -> InlinerStats {
    let mut stats = InlinerStats::default();

    // The set of callee names that are inlinable (computed once from the
    // pre-pass call graph/summaries). Bodies stay module-owned and are borrowed
    // live at the splice site, so bottom-up callee changes are visible to callers
    // without cloning a second body authority.
    let defined: Vec<String> = module.functions.iter().map(|f| f.name.clone()).collect();

    // Callees that an in-budget inline misses but whose inlining unlocks the
    // split-field deforestation (a caller hands them a non-escaping
    // `string_split_field` result). These are admitted on the safety gate alone
    // (over-budget but sound) — the targeted enabling the baton specifies.
    let split_enabled = split_field_enabled_callees(module, &defined);

    // Map function name -> index in the module vector for O(1) lookup.
    let index_of: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();

    // Record inlinable callees by module index, not by cloned body. The module
    // owns each body exactly once; call-site splicing borrows the caller mutably
    // and the callee immutably via `split_at_mut` below. That preserves the
    // bottom-up contract without a whole-module body snapshot.
    let inlinable_indices: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        // A callee whose canonical definition is linked externally (e.g. a
        // shared-stdlib-partition symbol that the native/wasm driver will
        // externalize into `stdlib_shared.o`) has external linkage: this module
        // does not own its body, so splicing a private copy at the call site is
        // unsound (it drops the external reference and forks the definition).
        // Refused unconditionally — the `Call` survives as an external reference.
        .filter(|(_, f)| !non_inlinable.contains(&f.name))
        .filter(|(_, f)| {
            is_inlineable(f, call_graph, summaries, tti)
                || (split_enabled.contains(&f.name) && is_inline_safe(f, call_graph))
        })
        .map(|(idx, f)| (f.name.clone(), idx))
        .collect();

    if inlinable_indices.is_empty() {
        return stats;
    }

    // Walk bottom-up over the SCC condensation: callees before callers.
    for scc in call_graph.bottom_up_order() {
        for caller_name in scc {
            let Some(&caller_idx) = index_of.get(&caller_name) else {
                continue;
            };

            // Collect this caller's inlinable call sites ONCE, then splice them
            // in **reverse** order (descending block id, then descending op
            // index). `collect_call_sites` yields ascending order, so `.rev()`
            // gives the splice-safe order: a splice at `(B, i)` keeps the
            // pre-call half at the *same* block id `B` with ops `0..i`, so every
            // not-yet-processed site at `(B, j<i)` or in an earlier block keeps
            // its `(block, op_index)` identity. Processing highest-index-first
            // therefore never invalidates a pending site's coordinates — no
            // re-collection needed.
            //
            // A refused site (refcount guard / shape mismatch) is simply skipped
            // (its `Call` survives, conservative-correct) and does NOT block the
            // remaining inlinable sites in the same caller.
            let mut changed_this_fn = false;
            let sites = {
                let caller = &module.functions[caller_idx];
                collect_call_sites(caller, &defined)
            };
            for site in sites.into_iter().rev() {
                if site.callee == caller_name {
                    continue; // self-call (recursive) — never inline.
                }
                let Some(&callee_idx) = inlinable_indices.get(&site.callee) else {
                    continue;
                };
                if callee_idx == caller_idx {
                    continue;
                }
                let (caller, callee) = if caller_idx < callee_idx {
                    let (left, right) = module.functions.split_at_mut(callee_idx);
                    (&mut left[caller_idx], &right[0])
                } else {
                    let (left, right) = module.functions.split_at_mut(caller_idx);
                    (&mut right[0], &left[callee_idx])
                };
                let callee_has_exception_handling = callee.has_exception_handling;
                let did_inline = splice_call_site(caller, callee, &site);
                if did_inline {
                    stats.sites_inlined += 1;
                    changed_this_fn = true;
                    // Propagate the callee's exception-handling flag. An
                    // OBSERVATION-only callee carries `has_exception_handling`
                    // (its `CheckException` ops set it); inlining its body imports
                    // those ops, so the merged caller must be flagged too — the
                    // conservative downstream passes (SCCP try-region, DCE) read
                    // this flag. (The caller is usually already flagged, since it
                    // has its own post-call `CheckException`, but a caller with no
                    // exception ops of its own would otherwise be left unflagged.)
                    if callee_has_exception_handling {
                        caller.has_exception_handling = true;
                    }
                }
            }

            if changed_this_fn {
                stats.functions_changed += 1;
                stats.changed_functions.push(caller_name.clone());
                // Re-run the per-function pipeline on the merged caller so the
                // inlined body is optimized jointly. A fresh PassManager (no
                // stale AnalysisManager cache) — run_pipeline builds one anew.
                // Bracket with type refinement on BOTH sides (refine → pipeline →
                // refine), matching every backend's per-function lift contract, so
                // `run_module_pipeline` returns every changed body *fully
                // type-refined*. The LLVM/WASM/native lowerers and the post-inline
                // representation-fact rebuild all depend on this invariant: an
                // unrefined merged body would floor its values to `DynBox` and emit
                // boxed dispatch on exactly the hot inlined paths.
                let caller = &mut module.functions[caller_idx];
                super::super::type_refine::refine_types(caller);
                let _ = super::run_pipeline(caller, tti);
                super::super::type_refine::refine_types(caller);
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    /// A callee `fn f(a, b) -> a + b` (single block, two params, one add,
    /// returns the sum).
    fn add_callee() -> TirFunction {
        let mut f = TirFunction::new(
            "addfn".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let p0 = ValueId(0);
        let p1 = ValueId(1);
        let sum = f.fresh_value();
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![p0, p1],
            results: vec![sum],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![sum] };
        f.value_types.insert(sum, TirType::I64);
        f
    }

    /// A CLOSURE callee shaped like the frontend's lowering of
    /// `def add(x): return base + x` capturing `base`: `param_names =
    /// ["__molt_closure__", "x"]`, body unpacks the captured env
    /// (`index [__molt_closure__, 0] -> cell; index [cell, 0] -> base`) and
    /// returns `base + x`. This is the exact shape that miscompiled (task #44):
    /// the splice would have bound `__molt_closure__` to the call's leading
    /// function-value operand, so `index [__molt_closure__, 0]` subscripts a
    /// function. `is_inlineable` must refuse it via the env-param marker.
    fn closure_callee(name: &str) -> TirFunction {
        let mut f = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::I64],
            TirType::I64,
        );
        // The production lift sets param_names from the frontend params; mirror
        // that here (TirFunction::new defaults to "p0"/"p1", test-only). The
        // FIRST param is the captured-environment marker -> this is a closure.
        f.param_names = vec![crate::MOLT_CLOSURE_PARAM_NAME.to_string(), "x".into()];
        let env = ValueId(0); // __molt_closure__
        let x = ValueId(1);
        let cell = f.fresh_value();
        let base = f.fresh_value();
        let sum = f.fresh_value();
        let entry = f.entry_block;
        let mut idx0a = AttrDict::new();
        idx0a.insert("value".into(), AttrValue::Int(0));
        let mut idx0b = AttrDict::new();
        idx0b.insert("value".into(), AttrValue::Int(0));
        let block = f.blocks.get_mut(&entry).unwrap();
        // cell = __molt_closure__[0]
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Index,
            operands: vec![env],
            results: vec![cell],
            attrs: idx0a,
            source_span: None,
        });
        // base = cell[0]
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Index,
            operands: vec![cell],
            results: vec![base],
            attrs: idx0b,
            source_span: None,
        });
        // sum = base + x
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![base, x],
            results: vec![sum],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![sum] };
        f.value_types.insert(cell, TirType::DynBox);
        f.value_types.insert(base, TirType::I64);
        f.value_types.insert(sum, TirType::I64);
        f
    }

    /// A const-returning leaf `fn k() -> 42`.
    fn const_callee() -> TirFunction {
        let mut f = TirFunction::new("constfn".into(), vec![], TirType::I64);
        let v = f.fresh_value();
        let entry = f.entry_block;
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(42));
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![v] };
        f.value_types.insert(v, TirType::I64);
        f
    }

    /// A caller `fn g() { x = const(); y = x + 1; return y }` that calls the
    /// const callee. The const arg list is empty; the result is `x`.
    fn caller_calling_const(callee_name: &str) -> TirFunction {
        let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
        let call_res = g.fresh_value();
        let one = g.fresh_value();
        let y = g.fresh_value();
        let entry = g.entry_block;
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str(callee_name.to_string()));
        let mut one_attrs = AttrDict::new();
        one_attrs.insert("value".into(), AttrValue::Int(1));
        let block = g.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![call_res],
            attrs: call_attrs,
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![one],
            attrs: one_attrs,
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![call_res, one],
            results: vec![y],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![y] };
        g
    }

    fn module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "m".into(),
            functions: funcs,
        }
    }

    fn analysis(m: &TirModule) -> (CallGraph, ModuleSummaries) {
        let cg = CallGraph::build(m);
        let sm = ModuleSummaries::compute(m, &cg);
        (cg, sm)
    }

    /// An **observation-only** callee `fn obs(a) -> a` shaped like real lowered
    /// TIR: an entry block carrying a `CheckException` (handler label
    /// `exc_label`) that, on a pending exception, routes to a void exception-exit
    /// block (`ret_void`, reached only via the exception edge); the normal path
    /// branches to a return block that yields the parameter. `has_exception_handling`
    /// is set (the `CheckException` would set it during lift) but there is NO
    /// handler region.
    fn observation_callee_with_type(name: &str, exc_label: i64, ty: TirType) -> TirFunction {
        let mut f = TirFunction::new(name.into(), vec![ty.clone()], ty.clone());
        f.has_exception_handling = true;
        let a = ValueId(0);
        let normal = f.fresh_block();
        let exc_exit = f.fresh_block();
        let entry = f.entry_block;
        {
            let mut ce_attrs = AttrDict::new();
            ce_attrs.insert("value".into(), AttrValue::Int(exc_label));
            let block = f.blocks.get_mut(&entry).unwrap();
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: ce_attrs,
                source_span: None,
            });
            block.terminator = Terminator::Branch {
                target: normal,
                args: vec![],
            };
        }
        f.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![a] },
            },
        );
        f.blocks.insert(
            exc_exit,
            TirBlock {
                id: exc_exit,
                args: vec![],
                ops: vec![],
                // ret_void — propagate the pending flag.
                terminator: Terminator::Return { values: vec![] },
            },
        );
        // The exception edge resolves through label_id_map: the exit block carries
        // the handler label the entry's CheckException references.
        f.label_id_map.insert(exc_exit.0, exc_label);
        f.value_types.insert(a, ty);
        f
    }

    fn observation_callee(name: &str, exc_label: i64) -> TirFunction {
        observation_callee_with_type(name, exc_label, TirType::I64)
    }

    /// A caller `fn c() { r = obs(5); <observe>; return r }` that calls an
    /// observation-only callee for a value, with its OWN post-call
    /// `CheckException` (handler label `caller_label`, resolving to the caller's
    /// own void exception-exit block). The caller's label deliberately COLLIDES
    /// numerically with the callee's exception label so the clone's fresh-label
    /// remap is exercised.
    fn caller_calling_obs_with_label(
        name: &str,
        callee_name: &str,
        caller_label: i64,
    ) -> TirFunction {
        let mut c = TirFunction::new(name.into(), vec![], TirType::I64);
        c.has_exception_handling = true;
        let five = c.fresh_value();
        let call_res = c.fresh_value();
        let caller_exit = c.fresh_block();
        let entry = c.entry_block;
        {
            let mut five_attrs = AttrDict::new();
            five_attrs.insert("value".into(), AttrValue::Int(5));
            let mut call_attrs = AttrDict::new();
            call_attrs.insert("s_value".into(), AttrValue::Str(callee_name.to_string()));
            let mut ce_attrs = AttrDict::new();
            ce_attrs.insert("value".into(), AttrValue::Int(caller_label));
            let block = c.blocks.get_mut(&entry).unwrap();
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![five],
                attrs: five_attrs,
                source_span: None,
            });
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![five],
                results: vec![call_res],
                attrs: call_attrs,
                source_span: None,
            });
            // The caller's own post-call exception observation.
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: ce_attrs,
                source_span: None,
            });
            block.terminator = Terminator::Return {
                values: vec![call_res],
            };
        }
        c.blocks.insert(
            caller_exit,
            TirBlock {
                id: caller_exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        c.label_id_map.insert(caller_exit.0, caller_label);
        c.value_types.insert(five, TirType::I64);
        c.value_types.insert(call_res, TirType::I64);
        c
    }

    /// Convenience: caller with a non-colliding label.
    fn caller_calling_obs(name: &str, callee_name: &str) -> TirFunction {
        caller_calling_obs_with_label(name, callee_name, 99)
    }

    // -- (a) clone + remap primitives ----------------------------------------

    #[test]
    fn clone_produces_disjoint_ids() {
        let callee = add_callee();
        let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
        // Two argument values already live in the caller.
        let a = caller.fresh_value();
        let b = caller.fresh_value();
        let before_next_value = caller.next_value;
        let before_next_block = caller.next_block;

        let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[a, b]);

        // The clone minted fresh value + block ids (the add result is fresh).
        assert!(caller.next_value > before_next_value, "value ids advanced");
        assert!(caller.next_block > before_next_block, "block ids advanced");
        // The cloned entry block exists and has NO args (params bound to a, b).
        let entry = &caller.blocks[&cloned.entry];
        assert!(
            entry.args.is_empty(),
            "cloned entry has no args (params bound)"
        );
        // The cloned Add uses the caller's arg values directly (a, b).
        let add = &entry.ops[0];
        assert_eq!(add.opcode, OpCode::Add);
        assert_eq!(add.operands, vec![a, b], "params bound directly to args");
        // The cloned add result is a fresh id, disjoint from a/b.
        assert!(add.results[0] != a && add.results[0] != b);
    }

    #[test]
    fn clone_entry_has_empty_args() {
        let callee = add_callee();
        let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
        let a = caller.fresh_value();
        let b = caller.fresh_value();
        let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[a, b]);
        assert!(caller.blocks[&cloned.entry].args.is_empty());
    }

    #[test]
    fn clone_transfers_all_loop_metadata() {
        // A callee with a header block carrying every loop-metadata kind.
        let mut callee = TirFunction::new("loopfn".into(), vec![], TirType::None);
        let header = callee.fresh_block();
        let end = callee.fresh_block();
        let cond = callee.fresh_block();
        // Give the entry a branch into the header; header/end/cond are trivial
        // blocks so the clone walk has them to remap.
        for bid in [header, end, cond] {
            callee.blocks.insert(
                bid,
                TirBlock {
                    id: bid,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }
        let entry = callee.entry_block;
        callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
        // Now wire all four loop maps + a label.
        callee.loop_roles.insert(header, LoopRole::LoopHeader);
        callee.loop_roles.insert(end, LoopRole::LoopEnd);
        callee.loop_pairs.insert(header, end);
        callee
            .loop_break_kinds
            .insert(header, LoopBreakKind::BreakIfTrue);
        callee.loop_cond_blocks.insert(header, cond);
        callee.label_id_map.insert(header.0, 7);

        let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
        let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[]);

        // All four maps + label_id_map must have one remapped entry each.
        assert_eq!(caller.loop_roles.len(), 2, "loop_roles transferred");
        assert_eq!(caller.loop_pairs.len(), 1, "loop_pairs transferred");
        assert_eq!(
            caller.loop_break_kinds.len(),
            1,
            "loop_break_kinds transferred"
        );
        assert_eq!(
            caller.loop_cond_blocks.len(),
            1,
            "loop_cond_blocks transferred"
        );
        assert_eq!(caller.label_id_map.len(), 1, "label_id_map transferred");
        // None of the transferred keys are the callee's original ids — they were
        // remapped to fresh caller block ids.
        assert!(!caller.loop_roles.contains_key(&header));
        assert!(!caller.loop_pairs.contains_key(&header));
        // The cloned entry is a fresh block (not the callee's BlockId(0)).
        assert!(cloned.entry != callee.entry_block || caller.next_block > callee.next_block);
    }

    // -- (b) splice ----------------------------------------------------------

    #[test]
    fn splice_removes_call_and_passes_verify() {
        let callee = const_callee();
        let mut caller = caller_calling_const("constfn");
        let site = collect_call_sites(&caller, &["constfn".to_string()]);
        assert_eq!(site.len(), 1);
        let did = splice_call_site(&mut caller, &callee, &site[0]);
        assert!(did, "splice succeeded");
        // No Call op remains anywhere.
        let remaining_calls: usize = caller
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(remaining_calls, 0, "the Call was eliminated");
        // The merged function is valid SSA.
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
    }

    #[test]
    fn splice_void_return() {
        // Callee returns nothing; caller calls it for effect.
        let mut callee = TirFunction::new("eff".into(), vec![], TirType::None);
        let entry = callee.entry_block;
        callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

        let mut caller = TirFunction::new("g".into(), vec![], TirType::None);
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str("eff".into()));
        let centry = caller.entry_block;
        let block = caller.blocks.get_mut(&centry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![],
            attrs: call_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };

        let sites = collect_call_sites(&caller, &["eff".to_string()]);
        assert_eq!(sites.len(), 1);
        assert!(splice_call_site(&mut caller, &callee, &sites[0]));
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("void-splice invalid: {e:?}"));
        let calls: usize = caller
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 0);
    }

    #[test]
    fn refcount_guard_refuses_arg_incref() {
        // Caller: IncRef(arg); call f(arg). The guard must refuse the splice.
        let mut callee = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let centry = callee.entry_block;
        callee.blocks.get_mut(&centry).unwrap().terminator = Terminator::Return { values: vec![] };

        let mut caller = TirFunction::new("g".into(), vec![TirType::DynBox], TirType::None);
        let arg = ValueId(0); // the caller's param
        let entry = caller.entry_block;
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str("f".into()));
        let block = caller.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::IncRef,
            operands: vec![arg],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![arg],
            results: vec![],
            attrs: call_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };

        let sites = collect_call_sites(&caller, &["f".to_string()]);
        assert_eq!(sites.len(), 1);
        assert!(
            !splice_call_site(&mut caller, &callee, &sites[0]),
            "refcount guard must refuse a site with arg IncRef before the call"
        );
        // The call survives intact.
        let calls: usize = caller
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 1, "refused site keeps its call");
    }

    // -- is_inlineable gates -------------------------------------------------

    #[test]
    fn recursive_not_inlined() {
        // f calls f → recursive.
        let mut f = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = f.entry_block;
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str("f".into()));
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let m = module(vec![f]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
    }

    #[test]
    fn too_large_not_inlined() {
        // A callee with op_count > budget.
        let mut f = TirFunction::new("big".into(), vec![], TirType::I64);
        let entry = f.entry_block;
        let tti = TargetInfo::native_release_fast();
        let budget = tti.inline_budget("big");
        // Allocate value ids first (avoid overlapping borrows).
        let vals: Vec<ValueId> = (0..budget + 5).map(|_| f.fresh_value()).collect();
        let block = f.blocks.get_mut(&entry).unwrap();
        for v in &vals {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(1));
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![*v],
                attrs,
                source_span: None,
            });
        }
        block.terminator = Terminator::Return {
            values: vec![vals[0]],
        };
        let m = module(vec![f]);
        let (cg, sm) = analysis(&m);
        assert!(
            !is_inlineable(&m.functions[0], &cg, &sm, &tti),
            "callee over budget is not inlinable"
        );
    }

    #[test]
    fn generator_not_inlined() {
        let mut f = TirFunction::new("gen".into(), vec![], TirType::None);
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Yield,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let m = module(vec![f]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
    }

    #[test]
    fn entry_predecessor_callee_not_inlined() {
        // A callee whose entry block is a branch target (a back-edge to entry)
        // cannot be spliced by the direct-param-binding model — refuse it.
        let mut f = TirFunction::new("looper".into(), vec![], TirType::None);
        let body = f.fresh_block();
        f.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![],
                // body branches BACK to the entry → entry has a predecessor.
                terminator: Terminator::Branch {
                    target: f.entry_block,
                    args: vec![],
                },
            },
        );
        let entry = f.entry_block;
        f.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
        let m = module(vec![f]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(
            !is_inlineable(&m.functions[0], &cg, &sm, &tti),
            "callee with entry-block predecessor is not inlinable this arc"
        );
    }

    #[test]
    fn handler_bearing_callee_not_inlined() {
        // A callee with a REAL exception handler region (TryStart/TryEnd) is
        // excluded — splicing across a handler boundary needs handler-label
        // re-targeting this arc does not perform.
        let mut f = TirFunction::new("guarded".into(), vec![], TirType::None);
        f.has_exception_handling = true;
        let entry = f.entry_block;
        {
            let block = f.blocks.get_mut(&entry).unwrap();
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::TryStart,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            });
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::TryEnd,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            });
            block.terminator = Terminator::Return { values: vec![] };
        }
        assert!(f.has_exception_handlers(), "TryStart/TryEnd => handlers");
        let m = module(vec![f]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
    }

    #[test]
    fn observation_only_callee_is_inlineable() {
        // An OBSERVATION-only callee (CheckException, no handler region) IS
        // inlinable even though `has_exception_handling` is set: it has no real
        // handler, so `has_exception_handlers()` is false.
        let callee = observation_callee("obs", 3);
        assert!(
            callee.has_exception_handling,
            "CheckException sets has_exception_handling"
        );
        assert!(
            !callee.has_exception_handlers(),
            "no TryStart/TryEnd/StateBlock => no handler region"
        );
        let caller = caller_calling_obs("c", "obs");
        let m = module(vec![callee, caller]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let obs = m.functions.iter().find(|f| f.name == "obs").unwrap();
        assert!(
            is_inlineable(obs, &cg, &sm, &tti),
            "observation-only callee is inlinable"
        );
    }

    #[test]
    fn closure_callee_not_inlined() {
        // task #44: a closure (first param == __molt_closure__) must NOT be
        // inlinable. The direct param->operand splice cannot bind the captured
        // env (it would bind the call's leading function-value operand instead),
        // so `is_inlineable` refuses it — conservative-correct exclusion.
        let callee = closure_callee("__main____add");
        assert!(
            is_closure(&callee),
            "first param is the env marker => closure"
        );
        let m = module(vec![callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(
            !is_inlineable(&m.functions[0], &cg, &sm, &tti),
            "closure callee must be refused (task #44 miscompile gate)"
        );
    }

    #[test]
    fn non_closure_same_arity_still_inlineable() {
        // The WIN must survive: a NON-closure 2-param callee (param_names do NOT
        // start with the env marker) is still inlinable. The closure gate keys on
        // the marker, not on arity, so a legitimate same-arity function is never
        // de-inlined by the fix.
        let callee = add_callee(); // params ["p0", "p1"] — not a closure
        assert!(
            !is_closure(&callee),
            "add_callee's first param is not the env marker"
        );
        let m = module(vec![callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(
            is_inlineable(&m.functions[0], &cg, &sm, &tti),
            "non-closure same-arity callee stays inlinable (perf win preserved)"
        );
    }

    #[test]
    fn run_inliner_refuses_closure_call_site() {
        // End-to-end through the production chokepoint: a caller that calls a
        // closure must NOT have the call spliced away. The Call op survives and
        // the closure body is NOT cloned into the caller (no Index over the
        // function value), so the miscompile cannot occur.
        let callee = closure_callee("__main____add");
        // caller g(): r = __main____add(<func>, 10); return r.
        // Operands model the real call ABI [callee_value, arg0].
        let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
        let func_val = g.fresh_value();
        let ten = g.fresh_value();
        let res = g.fresh_value();
        let entry = g.entry_block;
        let mut fattrs = AttrDict::new();
        fattrs.insert("value".into(), AttrValue::Int(0)); // stand-in producer
        let mut tattrs = AttrDict::new();
        tattrs.insert("value".into(), AttrValue::Int(10));
        let mut call_attrs = AttrDict::new();
        call_attrs.insert(
            "s_value".into(),
            AttrValue::Str("__main____add".to_string()),
        );
        let block = g.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![func_val],
            attrs: fattrs,
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ten],
            attrs: tattrs,
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![func_val, ten],
            results: vec![res],
            attrs: call_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![res] };
        g.value_types.insert(res, TirType::I64);

        let mut m = module(vec![g, callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
        assert_eq!(stats.sites_inlined, 0, "closure call site is NOT inlined");
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        let calls: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 1, "the Call op survives (closure not spliced)");
        // No Index op leaked into the caller (the closure body was not cloned).
        let indexes: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Index)
            .count();
        assert_eq!(indexes, 0, "closure body not cloned into caller");
    }

    // -- run_inliner end-to-end ----------------------------------------------

    #[test]
    fn run_inliner_inlines_const_call() {
        // g() { x = constfn(); return x + 1 }, constfn() = 42.
        // After inlining + re-running the pipeline, the Call is gone, the merged
        // function is valid SSA, and the callee's `const 42` now lives inside g
        // (the call boundary is eliminated). The downstream `const(42)+1 → 43`
        // arithmetic fold across the continuation block-argument is the
        // backend's / a future jump-threading pass's job — verified end-to-end
        // by the differential test, not asserted here (the current per-function
        // pipeline has no single-predecessor block-coalescing pass).
        let callee = const_callee();
        let caller = caller_calling_const("constfn");
        let mut m = module(vec![caller, callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
        assert_eq!(stats.sites_inlined, 1, "one site inlined");
        assert_eq!(stats.functions_changed, 1, "g changed");
        // No Call op remains in g.
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        let calls: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 0, "constfn call eliminated from g");
        // g is valid SSA after the pipeline re-run.
        crate::tir::verify::verify_function(g)
            .unwrap_or_else(|e| panic!("g invalid after inlining: {e:?}"));
        // The inlined callee's `const 42` is now part of g's body.
        let has_const_42 = g.blocks.values().any(|b| {
            b.ops.iter().any(|op| {
                op.opcode == OpCode::ConstInt
                    && matches!(op.attrs.get("value"), Some(AttrValue::Int(42)))
            })
        });
        assert!(has_const_42, "callee's const 42 inlined into g");
    }

    #[test]
    fn clone_attrs_without_simple_names_drops_only_value_names() {
        // The strip helper drops `_simple_out` and `_simple_result_N` (the
        // collision-prone SimpleIR value-name annotations) but preserves every
        // other attribute verbatim (e.g. the call symbol or a const value).
        let mut attrs = AttrDict::new();
        attrs.insert("_simple_out".into(), AttrValue::Str("x".into()));
        attrs.insert("_simple_result_0".into(), AttrValue::Str("y".into()));
        attrs.insert("_simple_result_1".into(), AttrValue::Str("z".into()));
        attrs.insert("s_value".into(), AttrValue::Str("callee".into()));
        attrs.insert("value".into(), AttrValue::Int(7));
        let stripped = clone_attrs_without_simple_names(&attrs);
        assert!(!stripped.contains_key("_simple_out"), "_simple_out dropped");
        assert!(
            !stripped.contains_key("_simple_result_0"),
            "_simple_result_0 dropped"
        );
        assert!(
            !stripped.contains_key("_simple_result_1"),
            "_simple_result_1 dropped"
        );
        assert_eq!(
            stripped.get("s_value"),
            Some(&AttrValue::Str("callee".into())),
            "s_value preserved"
        );
        assert_eq!(
            stripped.get("value"),
            Some(&AttrValue::Int(7)),
            "value preserved"
        );
    }

    #[test]
    fn inlined_ops_do_not_inherit_callee_simple_out_names() {
        // A callee whose op carries `_simple_out: "collide"` is inlined into a
        // caller that has its OWN op with the SAME `_simple_out: "collide"`. After
        // inlining, the name must appear on exactly ONE op (the caller's
        // original): the cloned callee op must have shed the name, so a name-keyed
        // container-dispatch lookup cannot resolve the inlined value to the
        // caller's kind. Before the strip, the merged body had two ops named
        // "collide" — a latent miscompile.
        let mut callee = TirFunction::new("c".into(), vec![], TirType::I64);
        let cv = callee.fresh_value();
        {
            let entry = callee.entry_block;
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(1));
            attrs.insert("_simple_out".into(), AttrValue::Str("collide".into()));
            let block = callee.blocks.get_mut(&entry).unwrap();
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![cv],
                attrs,
                source_span: None,
            });
            block.terminator = Terminator::Return { values: vec![cv] };
        }
        callee.value_types.insert(cv, TirType::I64);

        // caller g(): own = const 9 (named "collide"); r = c(); return own.
        let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
        let own = g.fresh_value();
        let call_res = g.fresh_value();
        {
            let entry = g.entry_block;
            let mut own_attrs = AttrDict::new();
            own_attrs.insert("value".into(), AttrValue::Int(9));
            own_attrs.insert("_simple_out".into(), AttrValue::Str("collide".into()));
            let mut call_attrs = AttrDict::new();
            call_attrs.insert("s_value".into(), AttrValue::Str("c".into()));
            let block = g.blocks.get_mut(&entry).unwrap();
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![own],
                attrs: own_attrs,
                source_span: None,
            });
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![call_res],
                attrs: call_attrs,
                source_span: None,
            });
            block.terminator = Terminator::Return { values: vec![own] };
        }
        g.value_types.insert(own, TirType::I64);
        g.value_types.insert(call_res, TirType::I64);

        let mut m = module(vec![g, callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
        assert_eq!(stats.sites_inlined, 1, "c() inlined into g");
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        let collide_count: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.attrs.get("_simple_out") == Some(&AttrValue::Str("collide".into())))
            .count();
        assert_eq!(
            collide_count, 1,
            "only the caller's own op keeps the name; the inlined op shed it"
        );
    }

    #[test]
    fn run_inliner_inlines_add_call_with_args() {
        // g(p, q) { return addfn(p, q) }, addfn(a, b) = a + b.
        let callee = add_callee();
        let mut g = TirFunction::new("g".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let p = ValueId(0);
        let q = ValueId(1);
        let res = g.fresh_value();
        let entry = g.entry_block;
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str("addfn".into()));
        let block = g.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![p, q],
            results: vec![res],
            attrs: call_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![res] };

        let mut m = module(vec![g, callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
        assert_eq!(stats.sites_inlined, 1);
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        let calls: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 0, "addfn call eliminated");
        crate::tir::verify::verify_function(g).unwrap_or_else(|e| panic!("g invalid: {e:?}"));
        // The inlined body's Add (a+b with a=p, b=q) is present and uses the
        // caller's params directly.
        let add_uses_params = g.blocks.values().any(|b| {
            b.ops
                .iter()
                .any(|op| op.opcode == OpCode::Add && op.operands == vec![p, q])
        });
        assert!(add_uses_params, "inlined add uses caller params directly");
    }

    #[test]
    fn run_inliner_two_sites_same_block_both_inlined() {
        // g() { x = constfn(); y = constfn(); return x + y } — two calls to the
        // same inlinable leaf in one block. The reverse-order driver must splice
        // BOTH (a refused/early site must not block the other). After inlining,
        // zero Call ops remain and SSA is valid.
        let callee = const_callee();
        let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
        let x = g.fresh_value();
        let y = g.fresh_value();
        let sum = g.fresh_value();
        let entry = g.entry_block;
        let mk_call = |name: &str, out: ValueId| {
            let mut a = AttrDict::new();
            a.insert("s_value".into(), AttrValue::Str(name.to_string()));
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![out],
                attrs: a,
                source_span: None,
            }
        };
        let block = g.blocks.get_mut(&entry).unwrap();
        block.ops.push(mk_call("constfn", x));
        block.ops.push(mk_call("constfn", y));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![x, y],
            results: vec![sum],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![sum] };

        let mut m = module(vec![g, callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
        assert_eq!(stats.sites_inlined, 2, "both call sites inlined");
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        let calls: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 0, "both constfn calls eliminated");
        crate::tir::verify::verify_function(g)
            .unwrap_or_else(|e| panic!("g invalid after 2-site inlining: {e:?}"));
        // Two distinct const-42 ops now live in g (one per inlined site).
        let const_42_count: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| {
                op.opcode == OpCode::ConstInt
                    && matches!(op.attrs.get("value"), Some(AttrValue::Int(42)))
            })
            .count();
        assert_eq!(
            const_42_count, 2,
            "each inlined site contributes a const 42"
        );
    }

    // -- (c) exception-observation inlining ----------------------------------

    /// The set of every label value in `func`'s `label_id_map` plus every
    /// exception-op `"value"` label, used to assert collision-freedom.
    fn all_labels(func: &TirFunction) -> Vec<i64> {
        function_label_ids(func).into_iter().collect()
    }

    #[test]
    fn splice_observation_callee_remaps_labels_collision_free() {
        // Callee exception label 3; caller ALSO uses label 3 (collision). After
        // splicing, the cloned exit block must carry a FRESH label (not 3), the
        // caller's original label 3 must survive, and no two blocks may share a
        // label value (which would make `exception_label_to_block` ambiguous and
        // emit duplicate `label N` ops in lower_to_simple — a miscompile).
        let callee = observation_callee("obs", 3);
        let mut caller = caller_calling_obs_with_label("c", "obs", 3);

        let sites = collect_call_sites(&caller, &["obs".to_string()]);
        assert_eq!(sites.len(), 1);
        assert!(splice_call_site(&mut caller, &callee, &sites[0]), "spliced");

        // No two blocks share a label value.
        let labels: Vec<i64> = caller.label_id_map.values().copied().collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            labels.len(),
            "every block label is distinct (no collision): {labels:?}"
        );
        // The caller's original label 3 survived.
        assert!(all_labels(&caller).contains(&3), "caller label 3 preserved");

        // Every cloned CheckException's handler label resolves to a block that
        // carries that exact label in label_id_map (the exception edge resolves).
        let label_to_block: std::collections::HashMap<i64, BlockId> = caller
            .label_id_map
            .iter()
            .map(|(b, l)| (*l, BlockId(*b)))
            .collect();
        for block in caller.blocks.values() {
            for op in &block.ops {
                if let Some(label) = exception_label_of(op) {
                    assert!(
                        label_to_block.contains_key(&label),
                        "CheckException label {label} resolves to a block"
                    );
                }
            }
        }
        // The merged function is valid SSA.
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
        // The Call is gone.
        let calls: usize = caller
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 0, "obs call eliminated");
    }

    #[test]
    fn splice_void_exception_exit_pads_placeholder() {
        // The observation callee's exception-exit returns NO value, but the call
        // wants one. The splice must NOT refuse (that would re-dormant the inliner
        // on every value-returning observation callee) — it pads the continuation
        // arg with a representation-matched dead placeholder. The exit branch ends
        // up supplying exactly one continuation arg, and the merged fn verifies.
        let callee = observation_callee("obs", 3);
        let mut caller = caller_calling_obs("c", "obs");

        let sites = collect_call_sites(&caller, &["obs".to_string()]);
        assert_eq!(sites.len(), 1);
        assert!(
            splice_call_site(&mut caller, &callee, &sites[0]),
            "value-returning observation callee inlines (not refused)"
        );
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("merged fn invalid SSA after placeholder pad: {e:?}"));

        // Find the continuation block: the one whose single arg is the original
        // call result. Every block that branches to it must supply exactly 1 arg
        // (the normal-return value OR the placeholder). At least two predecessors
        // exist (normal return + exception exit), and the exception-exit
        // predecessor ends in a placeholder const op feeding its branch arg.
        let mut placeholder_const_seen = false;
        for block in caller.blocks.values() {
            if let Terminator::Branch { args, .. } = &block.terminator {
                // A 1-op cloned exit block ending in a Branch with 1 arg whose
                // value is produced by a trailing Const op is the padded exit.
                if args.len() == 1
                    && block
                        .ops
                        .last()
                        .map(|op| {
                            let expected = dead_placeholder_const_for_type(&TirType::I64, args[0]);
                            op.opcode == expected.opcode
                                && op.operands == expected.operands
                                && op.results == expected.results
                                && op.attrs == expected.attrs
                        })
                        .unwrap_or(false)
                {
                    placeholder_const_seen = true;
                }
            }
        }
        assert!(
            placeholder_const_seen,
            "the void exception-exit branch is padded with a placeholder const"
        );
    }

    #[test]
    fn run_inliner_inlines_observation_callee_end_to_end() {
        // End-to-end through run_inliner (clone + splice + per-function pipeline
        // re-run): a value-returning observation-only callee is inlined, the Call
        // is gone, and the merged caller is valid SSA with collision-free labels.
        let callee = observation_callee("obs", 3);
        let caller = caller_calling_obs_with_label("c", "obs", 3);
        let mut m = module(vec![caller, callee]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
        assert_eq!(stats.sites_inlined, 1, "obs inlined into c");
        let c = m.functions.iter().find(|f| f.name == "c").unwrap();
        let calls: usize = c
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 0, "obs call eliminated from c");
        crate::tir::verify::verify_function(c)
            .unwrap_or_else(|e| panic!("c invalid after observation inlining: {e:?}"));
        // Labels remain collision-free after the pipeline re-run.
        let labels: Vec<i64> = c.label_id_map.values().copied().collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "labels distinct: {labels:?}");
    }
}
