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
//! Phases c (exception-bearing callees), d (cost / multi-site / fixed-point),
//! and e (retire the SimpleIR inliner) are SEPARATE later arcs. This arc is a
//! complete structural piece: [`is_inlineable`] conservatively refuses any
//! callee with `has_exception_handling` (phase c), any recursive-SCC member, any
//! callee over the cost-model op budget, and any callee containing a
//! generator/async op. Refusing exception-bearing callees is *conservative-
//! correct*, not interim: it never miscompiles, it only forgoes an optimization
//! the later arc unlocks.
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

use std::collections::HashMap;

use super::super::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use super::super::call_graph::CallGraph;
use super::super::function::{TirFunction, TirModule};
use super::super::ops::{AttrValue, OpCode, TirOp};
use super::super::target_info::TargetInfo;
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
}

/// Read an op's `s_value` string attribute, if present.
fn s_value(op: &TirOp) -> Option<&str> {
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
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
    matches!(
        opcode,
        OpCode::AllocTask
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            | OpCode::Yield
            | OpCode::YieldFrom
            | OpCode::StateBlockStart
            | OpCode::StateBlockEnd
    )
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
/// * **exception-bearing** — `has_exception_handling` (phase c handles these).
/// * **entry block has predecessors** — the splice binds parameters *directly*
///   to the call arguments and clones the callee entry as an argument-less
///   block. That is only SSA-valid when no branch targets the entry (i.e. the
///   entry has zero predecessors — the normal case, since the SSA lift puts
///   loop headers in separate blocks). A callee whose entry block is itself a
///   branch target would need its entry args preserved, which this arc's
///   direct-binding splice does not model, so it is refused (never miscompiled).
pub fn is_inlineable(
    callee: &TirFunction,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
) -> bool {
    if call_graph.recursive_set().contains(&callee.name) {
        return false;
    }
    if callee.has_exception_handling {
        // Phase c. Conservative-correct exclusion this arc.
        return false;
    }
    // Defensive re-scan: never inline a body that carries a state-machine op
    // even if `has_exception_handling` did not catch it (e.g. a bare `Yield`
    // without a TryStart). Cheap and fail-closed.
    if callee
        .blocks
        .values()
        .any(|b| b.ops.iter().any(|op| is_generator_or_async_op(op.opcode)))
    {
        return false;
    }
    if entry_block_has_predecessor(callee) {
        return false;
    }
    let op_count = summaries
        .get(&callee.name)
        .map(|s| s.op_count)
        .unwrap_or_else(|| callee.blocks.values().map(|b| b.ops.len()).sum());
    if op_count > tti.inline_budget(&callee.name) {
        return false;
    }
    true
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
        Terminator::Switch { cases, default, .. } => {
            *default == entry || cases.iter().any(|(_, t, _)| *t == entry)
        }
        Terminator::Return { .. } | Terminator::Unreachable => false,
    })
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
fn clone_function_body_with_fresh_ids(
    callee: &TirFunction,
    caller: &mut TirFunction,
    arg_values: &[ValueId],
) -> ClonedCallee {
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
    let fresh_for = |old: ValueId, value_map: &mut HashMap<ValueId, ValueId>, caller: &mut TirFunction| -> ValueId {
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

        // Cloned ops with operands/results remapped.
        let new_ops: Vec<TirOp> = src
            .ops
            .iter()
            .map(|op| TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands: op.operands.iter().map(|v| remap(*v, &value_map)).collect(),
                results: op.results.iter().map(|v| remap(*v, &value_map)).collect(),
                attrs: op.attrs.clone(),
                source_span: op.source_span,
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
    // back-conversion.
    transfer_loop_metadata(callee, caller, &block_map);

    ClonedCallee {
        entry: remap_block(callee.entry_block),
        cloned_blocks: callee_block_ids.iter().map(|b| remap_block(*b)).collect(),
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
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

/// Transfer `label_id_map` + `loop_roles` + `loop_pairs` + `loop_break_kinds` +
/// `loop_cond_blocks` from the callee into the caller, remapping every block-id
/// key (and any block-id-valued entry) through `block_map`.
fn transfer_loop_metadata(
    callee: &TirFunction,
    caller: &mut TirFunction,
    block_map: &HashMap<BlockId, BlockId>,
) {
    // label_id_map is keyed by BlockId.0 (a raw u32). Remap the key through the
    // block map so the cloned exception/jump targets resolve to the original
    // label ids in the merged function.
    for (old_block_u32, label_val) in &callee.label_id_map {
        if let Some(new_bid) = block_map.get(&BlockId(*old_block_u32)) {
            caller.label_id_map.entry(new_bid.0).or_insert(*label_val);
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
            caller.loop_cond_blocks.entry(*new_header).or_insert(*new_cond);
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

    // Return-arity compatibility pre-check (BEFORE any mutation, so a refusal
    // leaves `caller` byte-identical — no fragile mid-splice rollback). The
    // continuation block will carry one argument iff the call produces a value.
    // Every callee `Return` must then carry a matching value count: a value call
    // demands exactly one returned value from each return site; a void call
    // tolerates any (the returned value, if any, is discarded). A callee that
    // returns *no* value at some site while the call expects one is a
    // frontend-shape mismatch we refuse rather than fabricate a value for.
    let call_wants_value = call_result.is_some();
    if call_wants_value {
        for block in callee.blocks.values() {
            if let Terminator::Return { values } = &block.terminator {
                if values.is_empty() {
                    return false;
                }
            }
        }
    }

    // Clone the callee body into the caller (params → call args).
    let cloned = clone_function_body_with_fresh_ids(callee, caller, &call_args);

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
    // Arity is guaranteed compatible by the pre-check above: a value call's
    // continuation has exactly one arg and every callee return carries ≥1 value
    // (take the first — the single-return convention); a void call's
    // continuation has zero args (drop any returned value, which the call
    // discarded). No rollback path is reachable here.
    let cont_arity = cont_args.len();
    debug_assert!(cont_arity <= 1, "continuation arity is 0 (void) or 1 (value)");
    for &cloned_bid in &cloned.cloned_blocks {
        let block = caller
            .blocks
            .get_mut(&cloned_bid)
            .expect("cloned block missing");
        if let Terminator::Return { values } = &block.terminator {
            let branch_args: Vec<ValueId> = if cont_arity == 1 {
                debug_assert!(
                    !values.is_empty(),
                    "value-call return must carry a value (pre-checked)"
                );
                vec![values[0]]
            } else {
                Vec::new()
            };
            block.terminator = Terminator::Branch {
                target: cont_block_id,
                args: branch_args,
            };
        }
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
        if let Terminator::Return { values } = &block.terminator {
            if let Some(v) = values.first() {
                if let Some(ty) = callee.value_types.get(v) {
                    return Some(ty.clone());
                }
            }
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
) -> InlinerStats {
    let mut stats = InlinerStats::default();

    // The set of callee names that are inlinable (computed once over the
    // pre-pass bodies — phase a/b inlines a single bottom-up sweep, no
    // fixed-point, so callee bodies do not change under us during the sweep).
    let defined: Vec<String> = module.functions.iter().map(|f| f.name.clone()).collect();

    // Snapshot every inlinable callee body up front (owned clones), keyed by
    // name. This sidesteps the borrow-checker disjointness problem: the splice
    // reads the callee from this snapshot while holding `&mut` on the caller in
    // the module vector. The snapshot is the pre-sweep body, which is exactly
    // the bottom-up contract (callees finalized before callers; a callee is not
    // re-inlined into after being snapshotted because the sweep visits each SCC
    // once).
    let inlinable_bodies: HashMap<String, TirFunction> = module
        .functions
        .iter()
        .filter(|f| is_inlineable(f, call_graph, summaries, tti))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    if inlinable_bodies.is_empty() {
        return stats;
    }

    // Map function name -> index in the module vector for O(1) lookup.
    let index_of: HashMap<&str, usize> = module
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect::<HashMap<_, _>>();
    // Own the indices (drop the borrow on `module.functions` before mutation).
    let index_of: HashMap<String, usize> = index_of
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

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
                let Some(callee) = inlinable_bodies.get(&site.callee) else {
                    continue;
                };
                // Clone the callee snapshot so the borrow on the map does not
                // overlap the `&mut module.functions[caller_idx]`.
                let callee_owned = callee.clone();
                let caller = &mut module.functions[caller_idx];
                let did_inline = splice_call_site(caller, &callee_owned, &site);
                if did_inline {
                    stats.sites_inlined += 1;
                    changed_this_fn = true;
                    // Propagate the callee's exception-handling flag (it is
                    // false here — phase c excludes EH callees — but keep the
                    // contract explicit for when phase c lands).
                    if callee_owned.has_exception_handling {
                        caller.has_exception_handling = true;
                    }
                }
            }

            if changed_this_fn {
                stats.functions_changed += 1;
                // Re-run the per-function pipeline on the merged caller so the
                // inlined body is optimized jointly. A fresh PassManager (no
                // stale AnalysisManager cache) — run_pipeline builds one anew.
                let caller = &mut module.functions[caller_idx];
                super::super::type_refine::refine_types(caller);
                let _ = super::run_pipeline(caller, tti);
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    /// A callee `fn f(a, b) -> a + b` (single block, two params, one add,
    /// returns the sum).
    fn add_callee() -> TirFunction {
        let mut f = TirFunction::new("addfn".into(), vec![TirType::I64, TirType::I64], TirType::I64);
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
        assert!(entry.args.is_empty(), "cloned entry has no args (params bound)");
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
        callee.blocks.get_mut(&entry).unwrap().terminator =
            Terminator::Branch { target: header, args: vec![] };
        // Now wire all four loop maps + a label.
        callee.loop_roles.insert(header, LoopRole::LoopHeader);
        callee.loop_roles.insert(end, LoopRole::LoopEnd);
        callee.loop_pairs.insert(header, end);
        callee.loop_break_kinds.insert(header, LoopBreakKind::BreakIfTrue);
        callee.loop_cond_blocks.insert(header, cond);
        callee.label_id_map.insert(header.0, 7);

        let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
        let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[]);

        // All four maps + label_id_map must have one remapped entry each.
        assert_eq!(caller.loop_roles.len(), 2, "loop_roles transferred");
        assert_eq!(caller.loop_pairs.len(), 1, "loop_pairs transferred");
        assert_eq!(caller.loop_break_kinds.len(), 1, "loop_break_kinds transferred");
        assert_eq!(caller.loop_cond_blocks.len(), 1, "loop_cond_blocks transferred");
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
        callee.blocks.get_mut(&entry).unwrap().terminator =
            Terminator::Return { values: vec![] };

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
        let mut callee =
            TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let centry = callee.entry_block;
        callee.blocks.get_mut(&centry).unwrap().terminator =
            Terminator::Return { values: vec![] };

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
        block.terminator = Terminator::Return { values: vec![vals[0]] };
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
                terminator: Terminator::Branch { target: f.entry_block, args: vec![] },
            },
        );
        let entry = f.entry_block;
        f.blocks.get_mut(&entry).unwrap().terminator =
            Terminator::Branch { target: body, args: vec![] };
        let m = module(vec![f]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(
            !is_inlineable(&m.functions[0], &cg, &sm, &tti),
            "callee with entry-block predecessor is not inlinable this arc"
        );
    }

    #[test]
    fn exception_bearing_not_inlined_this_arc() {
        // has_exception_handling callee is excluded (phase c).
        let mut f = TirFunction::new("guarded".into(), vec![], TirType::None);
        f.has_exception_handling = true;
        let entry = f.entry_block;
        f.blocks.get_mut(&entry).unwrap().terminator =
            Terminator::Return { values: vec![] };
        let m = module(vec![f]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
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
        let stats = run_inliner(&mut m, &cg, &sm, &tti);
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
        let stats = run_inliner(&mut m, &cg, &sm, &tti);
        assert_eq!(stats.sites_inlined, 1);
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        let calls: usize = g
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(calls, 0, "addfn call eliminated");
        crate::tir::verify::verify_function(g)
            .unwrap_or_else(|e| panic!("g invalid: {e:?}"));
        // The inlined body's Add (a+b with a=p, b=q) is present and uses the
        // caller's params directly.
        let add_uses_params = g.blocks.values().any(|b| {
            b.ops.iter().any(|op| {
                op.opcode == OpCode::Add && op.operands == vec![p, q]
            })
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
        let stats = run_inliner(&mut m, &cg, &sm, &tti);
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
        assert_eq!(const_42_count, 2, "each inlined site contributes a const 42");
    }
}
