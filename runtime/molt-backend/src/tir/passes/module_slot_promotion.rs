//! Module-slot promotion — scalar promotion (mem2reg) of module-dict slots
//! across natural loops (the bench_sum 16× root cause; design:
//! `docs/design/foundation/10_module-global-loop-promotion.md`).
//!
//! Module-level Python keeps every loop-carried variable in the module dict:
//! each iteration pays `ModuleGetAttr` + `ModuleSetAttr` (+ the boxed value
//! round-trip) per variable — ~200× the cost of the register-carried local the
//! optimizer already handles 4.6× faster than CPython. This pass rewrites each
//! qualifying loop so promoted slots are carried as **header block arguments**
//! (SSA phis): in-loop reads use the carried value, in-loop writes redefine it,
//! every loop exit stores the final value back once, and every in-loop
//! `CheckException` whose slot state is dirty is routed through a
//! **compensation block** that stores the values live at that program point
//! before continuing to the original handler — an exception observer sees
//! exactly the as-if-stored-per-iteration state (deoptimization-state
//! discipline). After promotion the carried value is an ordinary SSA loop phi,
//! so the existing value-range / `RawI64Safe` machinery applies unchanged —
//! promotion turns module-level loops into the function-local shape the rest of
//! the optimizer is already good at.
//!
//! ## Soundness gates (each refusal is conservative-correct: the loop simply
//! keeps its per-iteration dict traffic)
//!
//! * **Concurrent observers**: CPython permits another *thread* to observe
//!   module globals mid-loop. [`module_has_concurrency_markers`] scans the
//!   whole module for threading reachability (`molt_thread_*` intrinsic name
//!   strings, `threading`/`_thread` imports) and the pass is a module-wide
//!   no-op when any is found. Fail-closed.
//! * **Other dict observers in the loop**: any op whose
//!   [`MemRegion`](super::alias_analysis::MemRegion) may alias
//!   [`MemRegion::ModuleDict`](super::alias_analysis::MemRegion::ModuleDict)
//!   (opaque calls are `GenericHeap` and alias everything) disqualifies the
//!   loop — EXCEPT const-named module get/set ops on the same module object:
//!   module dicts are plain dicts, so ops on *distinct constant keys* are
//!   disjoint (key-precise refinement of the oracle's coarse `ModuleDict`
//!   region).
//! * **Dynamic module access**: a module op with a non-constant name (or a
//!   `ModuleDelGlobal*` / `ModuleGetGlobal`) may touch any ATTR slot → the
//!   containing FUNCTION is skipped entirely. (`ModuleCache*` ops operate on
//!   the separate `sys.modules` registry, not the attr dict — not wildcards.)
//! * **Entry availability (no speculation)**: a slot is promoted only when its
//!   value is available at loop entry without hoisting a may-raise load past
//!   the loop guard: either the **preheader block** ends with a barrier-free
//!   suffix containing a get/set of the slot (its SSA value seeds the phi), or
//!   the slot's first in-loop access is in the **header block** (the header
//!   executes on every entry, so a preheader load raises exactly where the
//!   first iteration's access would have).
//! * **Linear loop body**: every loop block has at most one in-loop successor
//!   (no internal joins), so renaming needs no internal phis — the only join
//!   is the header phi this pass inserts. (Loops with internal control flow
//!   are a later phase; refusal is a perf bail, never a miscompile.)
//! * **No outside uses**: no in-loop module-op result may be used outside the
//!   loop (LCSSA would be needed; refused instead).
//! * **State machines**: functions containing generator/async ops are skipped.

use std::collections::{HashMap, HashSet};

use super::super::blocks::{BlockId, Terminator, TirBlock};
use super::super::dominators::{
    CfgEdgePolicy, build_pred_map_with, collect_loop_blocks, compute_idoms_with, dominates,
    reachable_blocks_with,
};
use super::super::function::{TirFunction, TirModule};
use super::super::ops::{AttrValue, OpCode, TirOp};
use super::super::values::{TirValue, ValueId};
use super::alias_analysis::{AliasAnalysisResult, MemRegion};

/// Statistics from one [`run_module_slot_promotion`] invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromotionStats {
    /// Functions that had at least one loop promoted.
    pub functions_changed: usize,
    /// Total (loop, slot) promotions performed.
    pub slots_promoted: usize,
    /// In-loop `ModuleGetAttr`/`ModuleSetAttr` ops deleted.
    pub ops_eliminated: usize,
}

/// Threading / concurrency reachability scan over the whole module.
///
/// A concurrent thread may legally observe module globals mid-loop, so
/// promotion is refused module-wide when threads can exist. The sound Tier-0
/// criterion: threads spawn only through the `threading` / `_thread` module
/// functions (or a direct `molt_thread_*` intrinsic CALL), and using those
/// modules requires an `Import` op naming them somewhere in the compiled
/// program (stdlib-internal imports included — e.g. an app importing `asyncio`
/// pulls in its executor's `threading` import, which this scan sees). Mere
/// `molt_thread_*` NAME STRINGS do not count: the always-linked
/// `builtins.py`/`threading.py` wrapper bodies carry those strings in
/// annotations and `require_intrinsic` arguments, but the wrappers only run
/// behind the imports this scan already catches — counting strings would
/// refuse every program ever compiled (the same over-broad-string-heuristic
/// trap as the deleted `needs_inlining` gate).
fn module_has_concurrency_markers(module: &TirModule) -> bool {
    for func in &module.functions {
        for block in func.blocks.values() {
            for op in &block.ops {
                match op.opcode {
                    OpCode::Import | OpCode::ImportFrom => {
                        for key in ["s_value", "name"] {
                            if let Some(AttrValue::Str(s)) = op.attrs.get(key)
                                && (s == "threading" || s == "_thread")
                            {
                                return true;
                            }
                        }
                    }
                    // A direct intrinsic CALL to the thread machinery (no
                    // module import needed) — the callee symbol, not an
                    // argument string.
                    OpCode::Call | OpCode::CallBuiltin => {
                        for key in ["s_value", "name"] {
                            if let Some(AttrValue::Str(s)) = op.attrs.get(key)
                                && s.starts_with("molt_thread")
                            {
                                return true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    false
}

/// A `Copy` passthrough of a structural / debug-marker SimpleIR kind (`line`
/// numbers, `nop`, labels…) — position metadata with no memory semantics.
/// `is_plain_value_copy` deliberately rejects passthroughs (they are not value
/// copies), but they are not barriers either.
fn is_marker_passthrough(op: &TirOp) -> bool {
    if op.opcode != OpCode::Copy {
        return false;
    }
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(k)) => k == "line" || crate::tir::is_structural(k),
        _ => false,
    }
}

/// A const-named module-attr access inside (or before) a loop.
#[derive(Debug, Clone)]
struct SlotAccess {
    block: BlockId,
    op_index: usize,
    /// True for `ModuleSetAttr` (operands `[module, name, value]`), false for
    /// `ModuleGetAttr` (operands `[module, name]`, one result).
    is_set: bool,
    /// The stored value (set) or the loaded result (get).
    value: ValueId,
}

/// Resolve `func`'s single module-object root: every `ModuleGetAttr` /
/// `ModuleSetAttr` first operand must canonicalize (through transparent
/// copies, via the alias oracle) to ONE root value. Returns `None` (skip
/// function) when there are no module ops, multiple roots, or a root that is
/// not function-entry-stable (we require it to be an entry-block argument so
/// the object identity provably never changes mid-function).
fn single_module_root(func: &TirFunction, alias: &AliasAnalysisResult) -> Option<ValueId> {
    let mut root: Option<ValueId> = None;
    for block in func.blocks.values() {
        for op in &block.ops {
            if matches!(op.opcode, OpCode::ModuleGetAttr | OpCode::ModuleSetAttr) {
                let m = alias.root(*op.operands.first()?);
                match root {
                    None => root = Some(m),
                    Some(r) if r == m => {}
                    Some(_) => return None,
                }
            }
        }
    }
    let root = root?;
    let entry = &func.blocks[&func.entry_block];
    entry.args.iter().any(|a| a.id == root).then_some(root)
}

/// Map every `ConstStr` result in `func` to its string value (module-attr
/// names are `ConstStr` operands).
fn const_str_defs(func: &TirFunction) -> HashMap<ValueId, String> {
    let mut map = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstStr
                && let (Some(&r), Some(AttrValue::Str(s))) =
                    (op.results.first(), op.attrs.get("s_value"))
            {
                map.insert(r, s.clone());
            }
        }
    }
    map
}

/// The dynamic / wildcard module ops that can touch ANY module-ATTR slot. A
/// function containing one is skipped wholesale (conservative).
///
/// Deliberately NOT wildcards: `ModuleCacheGet`/`ModuleCacheSet`/
/// `ModuleCacheDel` operate on the global module CACHE (`sys.modules`
/// registration — every module chunk registers/unregisters itself), and
/// `ModuleGetName` reads the module's name field — neither touches the
/// module's ATTR dict, so they cannot alias promoted slots. (Inside a loop
/// they would still refuse promotion through the coarse `ModuleDict` region
/// barrier — conservative — but their routine presence at chunk entry/exit
/// must not disqualify the whole function: that inertness is exactly what the
/// refusal-reason instrument caught on `bench_sum__molt_module_chunk_1`.)
fn is_wildcard_module_op(op: &TirOp, names: &HashMap<ValueId, String>) -> bool {
    match op.opcode {
        OpCode::ModuleGetAttr | OpCode::ModuleSetAttr => {
            // Const-named accesses are precise; a non-const name is wildcard.
            op.operands.get(1).is_none_or(|n| !names.contains_key(n))
        }
        OpCode::ModuleGetGlobal | OpCode::ModuleDelGlobal | OpCode::ModuleDelGlobalIfPresent => {
            true
        }
        _ => false,
    }
}

/// Generator/async state-machine opcodes — functions containing one are
/// skipped (mirrors the inliner's exclusion).
fn is_state_machine_op(opcode: OpCode) -> bool {
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

/// One natural loop selected for promotion analysis.
struct LoopInfo {
    header: BlockId,
    blocks: HashSet<BlockId>,
    /// The unique preheader (the single predecessor of the header outside the
    /// loop). Loops with multiple entries are refused.
    preheader: BlockId,
    /// Loop blocks in the unique linear in-loop order starting at the header
    /// (every block has at most one in-loop successor).
    linear_order: Vec<BlockId>,
}

/// Discover innermost, single-preheader, linear-body natural loops over the
/// terminator-only CFG (module chunks carry no `loop_roles` — their loops are
/// jump-shaped — so headers are derived from dominance back-edges).
fn discover_loops(
    func: &TirFunction,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
    idoms: &HashMap<BlockId, Option<BlockId>>,
    dbg: &mut DebugLog,
) -> Vec<LoopInfo> {
    // Dead blocks (e.g. a vestigial loop-else that still carries a branch to
    // the header) must not affect discovery: an UNREACHABLE outside
    // predecessor would otherwise break the unique-preheader requirement for a
    // perfectly promotable loop. Reachability over the terminator-only CFG.
    let reachable = reachable_blocks_with(func, CfgEdgePolicy::TerminatorOnly);

    // Headers: reachable blocks with a reachable predecessor they dominate.
    let mut headers: Vec<BlockId> = func
        .blocks
        .keys()
        .copied()
        .filter(|h| reachable.contains(h))
        .filter(|&h| {
            pred_map.get(&h).is_some_and(|ps| {
                ps.iter()
                    .any(|&p| reachable.contains(&p) && dominates(h, p, idoms))
            })
        })
        .collect();
    headers.sort_by_key(|b| b.0);

    let mut loops = Vec::new();
    'next_header: for &header in &headers {
        let blocks = collect_loop_blocks(func, pred_map, idoms, header);
        // Innermost only: no other header inside this loop's body.
        if headers
            .iter()
            .any(|&h2| h2 != header && blocks.contains(&h2))
        {
            dbg.note(format!("{} loop@{:?}: not innermost", func.name, header));
            continue;
        }
        // Unique REACHABLE preheader (dead outside-preds are ignored).
        let outside_preds: Vec<BlockId> = pred_map
            .get(&header)
            .map(|ps| {
                ps.iter()
                    .copied()
                    .filter(|p| !blocks.contains(p) && reachable.contains(p))
                    .collect()
            })
            .unwrap_or_default();
        let [preheader] = outside_preds[..] else {
            dbg.note(format!(
                "{} loop@{:?}: refused ({} reachable outside preds, need exactly 1)",
                func.name,
                header,
                outside_preds.len()
            ));
            continue;
        };
        // Linear in-loop order: from the header, follow the unique in-loop
        // terminator successor; every loop block must be visited exactly once
        // and have ≤1 in-loop successor (no internal joins/splits inside).
        let mut order = vec![header];
        let mut seen: HashSet<BlockId> = [header].into();
        let mut cur = header;
        while order.len() < blocks.len() {
            let succs = terminator_successors(&func.blocks[&cur].terminator);
            let inside: Vec<BlockId> = succs
                .iter()
                .copied()
                .filter(|s| blocks.contains(s) && !seen.contains(s))
                .collect();
            let [next] = inside[..] else {
                dbg.note(format!(
                    "{} loop@{:?}: refused (non-linear body at {:?}: {} unseen in-loop succs)",
                    func.name,
                    header,
                    cur,
                    inside.len()
                ));
                continue 'next_header;
            };
            order.push(next);
            seen.insert(next);
            cur = next;
        }
        loops.push(LoopInfo {
            header,
            blocks,
            preheader,
            linear_order: order,
        });
    }
    loops
}

fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            let mut v: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
            v.push(*default);
            v
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Run promotion over every function in `module`. Returns the names of the
/// functions whose bodies changed (the caller re-optimizes + back-converts
/// exactly these, mirroring the inliner's `changed_functions` contract).
pub fn run_module_slot_promotion(module: &mut TirModule) -> (PromotionStats, Vec<String>) {
    let mut stats = PromotionStats::default();
    let mut changed_names = Vec::new();
    let mut dbg = DebugLog::from_env();

    if module_has_concurrency_markers(module) {
        dbg.note("module-wide refusal: concurrency markers (threading/_thread import or molt_thread_* call)");
        dbg.flush(&module.name);
        return (stats, changed_names);
    }

    for func in &mut module.functions {
        let promoted = promote_function(func, &mut stats, &mut dbg);
        if promoted {
            stats.functions_changed += 1;
            changed_names.push(func.name.clone());
        }
    }
    dbg.flush(&module.name);
    (stats, changed_names)
}

/// `MOLT_PROMOTE_DEBUG=1` refusal-reason log, written through the debug-artifact
/// channel (`<artifact-dir>/promotion/<module>.txt`) — backend stderr does not
/// surface through the CLI on successful builds, artifacts do. The instrument
/// that keeps a silently-inert activation diagnosable (the L4 / needs_inlining
/// lesson, institutionalized).
struct DebugLog {
    lines: Option<Vec<String>>,
}

impl DebugLog {
    fn from_env() -> Self {
        Self {
            lines: (std::env::var("MOLT_PROMOTE_DEBUG").as_deref() == Ok("1")).then(Vec::new),
        }
    }
    fn note(&mut self, msg: impl Into<String>) {
        if let Some(lines) = &mut self.lines {
            lines.push(msg.into());
        }
    }
    fn flush(&mut self, module_name: &str) {
        if let Some(lines) = &self.lines {
            let sanitized: String = module_name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let _ = crate::debug_artifacts::write_debug_artifact(
                format!("promotion/{sanitized}.txt"),
                lines.join("\n") + "\n",
            );
        }
    }
}

fn promote_function(
    func: &mut TirFunction,
    stats: &mut PromotionStats,
    dbg: &mut DebugLog,
) -> bool {
    // Cheap pre-filter: nothing to do without module ops.
    if !func.blocks.values().any(|b| {
        b.ops
            .iter()
            .any(|op| matches!(op.opcode, OpCode::ModuleGetAttr | OpCode::ModuleSetAttr))
    }) {
        return false;
    }
    // Skip state machines outright.
    if func
        .blocks
        .values()
        .any(|b| b.ops.iter().any(|op| is_state_machine_op(op.opcode)))
    {
        dbg.note(format!("{}: skip (state-machine ops)", func.name));
        return false;
    }

    let names = const_str_defs(func);
    // Wildcard module access anywhere → skip the function.
    if let Some(op) = func
        .blocks
        .values()
        .find_map(|b| b.ops.iter().find(|op| is_wildcard_module_op(op, &names)))
    {
        dbg.note(format!(
            "{}: skip (wildcard module op {:?})",
            func.name, op.opcode
        ));
        return false;
    }

    let alias = AliasAnalysisResult::compute(func);
    let Some(module_root) = single_module_root(func, &alias) else {
        dbg.note(format!(
            "{}: skip (no single entry-arg module root)",
            func.name
        ));
        return false;
    };

    let pred_map = build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly);
    let idoms = compute_idoms_with(func, &pred_map, CfgEdgePolicy::TerminatorOnly);
    let loops = discover_loops(func, &pred_map, &idoms, dbg);
    if loops.is_empty() {
        return false;
    }

    let mut changed = false;
    for lp in loops {
        changed |= promote_loop(func, &lp, module_root, &names, &alias, stats, dbg);
    }
    changed
}

/// The per-(loop, slot) classification gathered before any mutation.
struct LoopPlan {
    /// Promoted slot names in deterministic order.
    slots: Vec<String>,
    /// Per-slot seed value on the preheader edge.
    entry_values: Vec<ValueId>,
    /// In-loop accesses, per slot.
    accesses: Vec<Vec<SlotAccess>>,
    /// Preheader gets that must be INSERTED (header-accessed slots with no
    /// preheader access): (slot index, fresh name-const ValueId).
    hoisted_loads: Vec<(usize, ValueId)>,
}

fn promote_loop(
    func: &mut TirFunction,
    lp: &LoopInfo,
    module_root: ValueId,
    names: &HashMap<ValueId, String>,
    alias: &AliasAnalysisResult,
    stats: &mut PromotionStats,
    dbg: &mut DebugLog,
) -> bool {
    // ---- legality + classification (no mutation) ---------------------------
    let mut accesses_by_slot: HashMap<String, Vec<SlotAccess>> = HashMap::new();

    for &bid in &lp.linear_order {
        let block = &func.blocks[&bid];
        for (op_index, op) in block.ops.iter().enumerate() {
            match op.opcode {
                OpCode::ModuleGetAttr | OpCode::ModuleSetAttr => {
                    // Const-named (wildcards were rejected function-wide); must
                    // be on THE module root.
                    let m = alias.root(op.operands[0]);
                    if m != module_root {
                        dbg.note(format!(
                            "{} loop@{:?}: refused (module op on non-root object)",
                            func.name, lp.header
                        ));
                        return false;
                    }
                    let name = names[&op.operands[1]].clone();
                    let is_set = op.opcode == OpCode::ModuleSetAttr;
                    let value = if is_set {
                        op.operands[2]
                    } else {
                        op.results[0]
                    };
                    accesses_by_slot.entry(name).or_default().push(SlotAccess {
                        block: bid,
                        op_index,
                        is_set,
                        value,
                    });
                }
                OpCode::CheckException => {} // compensated, not a barrier
                _ => {
                    // Pure, movable ops (the licm-canonical S3 predicate) and
                    // plain value copies cannot observe or mutate any memory —
                    // never barriers. (The alias oracle's coarse taxonomy
                    // defaults unlisted ops like `Copy` to `GenericHeap`,
                    // which would otherwise alias everything.) Everything else
                    // barriers when its region may alias the module dict.
                    if super::effects::opcode_is_pure_movable(op.opcode)
                        || op.is_plain_value_copy()
                        || is_marker_passthrough(op)
                    {
                        continue;
                    }
                    if alias.region_of(op).may_alias(&MemRegion::ModuleDict) {
                        let orig = match op.attrs.get("_original_kind") {
                            Some(AttrValue::Str(k)) => format!(" (_original_kind={k})"),
                            _ => String::new(),
                        };
                        dbg.note(format!(
                            "{} loop@{:?}: refused (barrier op {:?}{} in loop)",
                            func.name, lp.header, op.opcode, orig
                        ));
                        return false;
                    }
                }
            }
        }
    }
    if accesses_by_slot.is_empty() {
        return false;
    }

    // No in-loop module-op result may be used outside the loop (LCSSA refusal).
    let in_loop_results: HashSet<ValueId> = accesses_by_slot
        .values()
        .flatten()
        .filter(|a| !a.is_set)
        .map(|a| a.value)
        .collect();
    for (bid, block) in &func.blocks {
        if lp.blocks.contains(bid) {
            continue;
        }
        let uses_outside = block
            .ops
            .iter()
            .any(|op| op.operands.iter().any(|v| in_loop_results.contains(v)))
            || terminator_uses(&block.terminator, &in_loop_results);
        if uses_outside {
            dbg.note(format!(
                "{} loop@{:?}: refused (in-loop module value used outside; LCSSA)",
                func.name, lp.header
            ));
            return false;
        }
    }

    // Entry availability per slot.
    let mut slots: Vec<String> = accesses_by_slot.keys().cloned().collect();
    slots.sort();
    // The entry-seed scan walks ops BACKWARDS across the straight-line chain
    // of blocks ending at the preheader (the lift often splits the set-up
    // block from the loop with tiny pass-through blocks, so the seeds live a
    // block or two back). Chain membership: walk back while the current block
    // has exactly one predecessor and that predecessor unconditionally falls
    // through (single successor). Ops are cloned so the per-slot walk below
    // can allocate fresh ids on `func` without a live borrow of `func.blocks`.
    let preheader_ops: Vec<TirOp> = {
        let pred_map = build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly);
        let mut chain = vec![lp.preheader];
        let mut cur = lp.preheader;
        loop {
            let preds = pred_map.get(&cur).map(Vec::as_slice).unwrap_or(&[]);
            let [single] = preds else { break };
            if terminator_successors(&func.blocks[single].terminator).len() != 1 {
                break;
            }
            chain.push(*single);
            cur = *single;
        }
        // Oldest block first, so a reversed scan sees the LAST access first.
        chain.reverse();
        chain
            .iter()
            .flat_map(|b| func.blocks[b].ops.iter().cloned())
            .collect()
    };
    let mut entry_values = Vec::new();
    let mut hoisted_loads = Vec::new();
    for (slot_idx, slot) in slots.iter().enumerate() {
        // Walk the preheader block backwards for the LAST access of this slot
        // with no ModuleDict-aliasing barrier after it.
        let mut found: Option<ValueId> = None;
        for op in preheader_ops.iter().rev() {
            match op.opcode {
                OpCode::ModuleGetAttr | OpCode::ModuleSetAttr => {
                    if alias.root(op.operands[0]) == module_root
                        && names.get(&op.operands[1]).map(String::as_str) == Some(slot.as_str())
                    {
                        found = Some(if op.opcode == OpCode::ModuleSetAttr {
                            op.operands[2]
                        } else {
                            op.results[0]
                        });
                        break;
                    }
                    // A different slot's const-named access: key-disjoint, keep
                    // walking.
                }
                OpCode::CheckException => {}
                _ if super::effects::opcode_is_pure_movable(op.opcode)
                    || op.is_plain_value_copy()
                    || is_marker_passthrough(op) => {}
                _ if alias.region_of(op).may_alias(&MemRegion::ModuleDict) => break,
                _ => {}
            }
        }
        match found {
            Some(v) => {
                entry_values.push(v);
            }
            None => {
                // Hoist a preheader load — legal ONLY when (a) the slot's
                // first in-loop access is in the header block (executes on
                // every entry, so the load raises exactly where iteration 1
                // would), AND (b) the function has no real exception handlers:
                // a hoisted load that raises is observed by the FIRST
                // CheckException after it rather than the one following the
                // original get, which under `try` handlers could route to a
                // DIFFERENT handler. Handler-free functions have a single
                // function-exit label, so routing is identical either way.
                if func.has_exception_handlers() {
                    dbg.note(format!(
                        "{} loop@{:?}: refused (hoisted load for '{}' in handler-bearing fn)",
                        func.name, lp.header, slot
                    ));
                    return false;
                }
                // The guaranteed-on-entry prefix: the linear blocks from the
                // header through the FIRST block that can leave the loop (an
                // in-loop CondBranch/Switch or an exit edge). Every block in
                // this prefix executes on every loop entry, so a preheader
                // load raises exactly where the first iteration's access
                // would. (The lift often leaves the header itself an empty
                // join and puts the condition gets one block later.)
                let mut guaranteed: HashSet<BlockId> = HashSet::new();
                for &b in &lp.linear_order {
                    guaranteed.insert(b);
                    let succs = terminator_successors(&func.blocks[&b].terminator);
                    let conditional =
                        succs.len() > 1 || succs.iter().any(|s| !lp.blocks.contains(s));
                    if conditional {
                        break;
                    }
                }
                let first_in_guaranteed = accesses_by_slot[slot]
                    .iter()
                    .min_by_key(|a| {
                        (
                            lp.linear_order.iter().position(|b| *b == a.block),
                            a.op_index,
                        )
                    })
                    .is_some_and(|a| guaranteed.contains(&a.block));
                if !first_in_guaranteed {
                    dbg.note(format!(
                        "{} loop@{:?}: refused (slot '{}' not entry-available)",
                        func.name, lp.header, slot
                    ));
                    return false;
                }
                // The name const must dominate the preheader insertion point.
                // The in-loop ConstStr does NOT; synthesize a fresh ConstStr in
                // the preheader instead (constants are position-free).
                let fresh_name = func.fresh_value();
                let fresh_load = func.fresh_value();
                hoisted_loads.push((slot_idx, fresh_name));
                entry_values.push(fresh_load);
            }
        }
    }

    let plan = LoopPlan {
        accesses: slots.iter().map(|s| accesses_by_slot[s].clone()).collect(),
        slots,
        entry_values,
        hoisted_loads,
    };

    apply_promotion(func, lp, module_root, &plan, stats);
    true
}

fn terminator_uses(term: &Terminator, set: &HashSet<ValueId>) -> bool {
    match term {
        Terminator::Branch { args, .. } => args.iter().any(|v| set.contains(v)),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            set.contains(cond)
                || then_args.iter().any(|v| set.contains(v))
                || else_args.iter().any(|v| set.contains(v))
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            set.contains(value)
                || default_args.iter().any(|v| set.contains(v))
                || cases
                    .iter()
                    .any(|(_, _, args)| args.iter().any(|v| set.contains(v)))
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            default_args.iter().any(|v| set.contains(v))
                || cases
                    .iter()
                    .any(|(_, _, args)| args.iter().any(|v| set.contains(v)))
        }
        Terminator::Return { values } => values.iter().any(|v| set.contains(v)),
        Terminator::Unreachable => false,
    }
}

/// Retarget every appearance of `header` in `term` to `args`-augmented form:
/// append `extra` to the arg list of each edge into the header.
fn append_args_on_edges_to(term: &mut Terminator, header: BlockId, extra: &[ValueId]) {
    match term {
        Terminator::Branch { target, args } if *target == header => {
            args.extend_from_slice(extra);
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == header {
                then_args.extend_from_slice(extra);
            }
            if *else_block == header {
                else_args.extend_from_slice(extra);
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, b, args) in cases.iter_mut() {
                if *b == header {
                    args.extend_from_slice(extra);
                }
            }
            if *default == header {
                default_args.extend_from_slice(extra);
            }
        }
        _ => {}
    }
}

fn apply_promotion(
    func: &mut TirFunction,
    lp: &LoopInfo,
    module_root: ValueId,
    plan: &LoopPlan,
    stats: &mut PromotionStats,
) {
    let n = plan.slots.len();
    stats.slots_promoted += n;

    // ---- 1. hoisted preheader loads (fresh ConstStr + ModuleGetAttr) -------
    // entry_values for hoisted slots were pre-allocated as fresh ids; define
    // them now at the end of the preheader.
    {
        let mut hoist_ops = Vec::new();
        for &(slot_idx, fresh_name) in &plan.hoisted_loads {
            let mut name_attrs = super::super::ops::AttrDict::new();
            name_attrs.insert(
                "s_value".into(),
                AttrValue::Str(plan.slots[slot_idx].clone()),
            );
            hoist_ops.push(TirOp {
                dialect: super::super::ops::Dialect::Molt,
                opcode: OpCode::ConstStr,
                operands: vec![],
                results: vec![fresh_name],
                attrs: name_attrs,
                source_span: None,
            });
            hoist_ops.push(TirOp {
                dialect: super::super::ops::Dialect::Molt,
                opcode: OpCode::ModuleGetAttr,
                operands: vec![module_root, fresh_name],
                results: vec![plan.entry_values[slot_idx]],
                attrs: super::super::ops::AttrDict::new(),
                source_span: None,
            });
        }
        let pre = func.blocks.get_mut(&lp.preheader).expect("preheader");
        pre.ops.extend(hoist_ops);
    }

    // ---- 2. header phi args -------------------------------------------------
    // One fresh carried value per slot; type = the entry value's known type
    // (DynBox floor keeps Repr sound — value_range re-proves on the merged
    // body during the post-pass re-pipeline).
    let carried: Vec<ValueId> = (0..n).map(|_| func.fresh_value()).collect();
    for (i, &cv) in carried.iter().enumerate() {
        let ty = func
            .value_types
            .get(&plan.entry_values[i])
            .cloned()
            .unwrap_or(super::super::types::TirType::DynBox);
        func.value_types.insert(cv, ty.clone());
        let header = func.blocks.get_mut(&lp.header).expect("header");
        header.args.push(TirValue { id: cv, ty });
    }

    // Preheader edge passes the entry values; back edges pass the (renamed)
    // latch values — appended AFTER renaming computes them (step 4).
    {
        let pre = func.blocks.get_mut(&lp.preheader).expect("preheader");
        append_args_on_edges_to(&mut pre.terminator, lp.header, &plan.entry_values);
    }

    // ---- 3. rename through the linear body ---------------------------------
    // cur[i] = the SSA value of slot i at the current program point.
    //
    // DIRTINESS IS LOOP-LEVEL, NOT POSITIONAL: on iteration ≥2 the back edge
    // makes ANY in-loop set reach EVERY program point in the loop, so a slot
    // with at least one set is dirty at every CheckException and every exit —
    // even ones that appear BEFORE the set in linear block order. (A positional
    // dirty bit would skip compensation for a check that precedes the set in
    // block order yet runs after it at runtime — a wrong-observable-state
    // miscompile on iteration 2+.) A never-set slot is never dirty (its dict
    // value is already correct) and needs no stores anywhere.
    let slot_dirty: Vec<bool> = plan
        .accesses
        .iter()
        .map(|accs| accs.iter().any(|a| a.is_set))
        .collect();
    let any_dirty = slot_dirty.iter().any(|&d| d);
    let mut cur: Vec<ValueId> = carried.clone();
    // value replacement map for deleted gets: old result -> current value.
    let mut replace: HashMap<ValueId, ValueId> = HashMap::new();
    // Per-block end values for back-edge args + exit-edge store-backs.
    let mut values_at_block_end: HashMap<BlockId, Vec<ValueId>> = HashMap::new();
    // Compensation blocks to create for dirty CheckExceptions.
    struct Compensation {
        check_block: BlockId,
        check_op_index: usize,
        values: Vec<ValueId>,
        original_label: i64,
        original_operands: Vec<ValueId>,
    }
    let mut compensations: Vec<Compensation> = Vec::new();

    let slot_index_of_access: HashMap<(BlockId, usize), usize> = plan
        .accesses
        .iter()
        .enumerate()
        .flat_map(|(i, accs)| accs.iter().map(move |a| ((a.block, a.op_index), i)))
        .collect();

    for &bid in &lp.linear_order {
        let block = func.blocks.get_mut(&bid).expect("loop block");
        let mut new_ops: Vec<TirOp> = Vec::with_capacity(block.ops.len());
        for (op_index, op) in block.ops.iter().enumerate() {
            if let Some(&slot) = slot_index_of_access.get(&(bid, op_index)) {
                if op.opcode == OpCode::ModuleSetAttr {
                    // Redefine the carried value; delete the store.
                    cur[slot] = op.operands[2];
                } else {
                    // Replace the get's result with the carried value; delete.
                    replace.insert(op.results[0], cur[slot]);
                }
                stats.ops_eliminated += 1;
                continue;
            }
            if op.opcode == OpCode::CheckException && any_dirty {
                let label = match op.attrs.get("value") {
                    Some(AttrValue::Int(l)) => *l,
                    _ => {
                        // No label → keep as-is (defensive; nothing to retarget).
                        new_ops.push(op.clone());
                        continue;
                    }
                };
                compensations.push(Compensation {
                    check_block: bid,
                    check_op_index: new_ops.len(),
                    values: cur.clone(),
                    original_label: label,
                    original_operands: op.operands.clone(),
                });
                // The op itself is rewritten in step 5 (fresh label, no
                // operands); push a placeholder clone for now.
                new_ops.push(op.clone());
                continue;
            }
            new_ops.push(op.clone());
        }
        block.ops = new_ops;
        values_at_block_end.insert(bid, cur.clone());
    }

    // Apply the get-result replacement everywhere in the loop (operands and
    // terminators; outside uses were refused pre-transform). Resolve chains
    // (get B replaced by get A's replacement).
    let resolve = |v: ValueId, replace: &HashMap<ValueId, ValueId>| -> ValueId {
        let mut cur = v;
        while let Some(&next) = replace.get(&cur) {
            cur = next;
        }
        cur
    };
    for &bid in &lp.linear_order {
        let block = func.blocks.get_mut(&bid).expect("loop block");
        for op in &mut block.ops {
            for v in &mut op.operands {
                *v = resolve(*v, &replace);
            }
        }
        rewrite_terminator_values(&mut block.terminator, &|v| resolve(v, &replace));
    }
    // Carried values may appear in the recorded states; resolve them too.
    for vals in values_at_block_end.values_mut() {
        for v in vals.iter_mut() {
            *v = resolve(*v, &replace);
        }
    }
    for c in &mut compensations {
        for v in &mut c.values {
            *v = resolve(*v, &replace);
        }
        for v in &mut c.original_operands {
            *v = resolve(*v, &replace);
        }
    }

    // ---- 4. back edges + exit edges -----------------------------------------
    // Back edges (in-loop preds of the header): append the latch-end values.
    let in_loop_header_preds: Vec<BlockId> = lp
        .linear_order
        .iter()
        .copied()
        .filter(|b| terminator_successors(&func.blocks[b].terminator).contains(&lp.header))
        .collect();
    for pred in &in_loop_header_preds {
        let vals = values_at_block_end[pred].clone();
        let block = func.blocks.get_mut(pred).expect("latch");
        append_args_on_edges_to(&mut block.terminator, lp.header, &vals);
    }

    // Stray header predecessors: UNREACHABLE blocks (e.g. a vestigial
    // loop-else) may still physically branch to the header. Discovery ignored
    // them (correctly — they never execute), but the verifier checks branch
    // ARITY on every block regardless of reachability, so their edges must be
    // padded too. The entry seeds are well-formed dominating-exempt values for
    // a dead edge; semantics are unaffected.
    let updated: HashSet<BlockId> = in_loop_header_preds
        .iter()
        .copied()
        .chain([lp.preheader])
        .collect();
    let stray_preds: Vec<BlockId> = func
        .blocks
        .iter()
        .filter(|(bid, b)| {
            !updated.contains(bid) && terminator_successors(&b.terminator).contains(&lp.header)
        })
        .map(|(bid, _)| *bid)
        .collect();
    for pred in stray_preds {
        let block = func.blocks.get_mut(&pred).expect("stray pred");
        append_args_on_edges_to(&mut block.terminator, lp.header, &plan.entry_values);
    }

    // Exit edges: split each in-loop→outside edge with a store-back block for
    // the dirty slots at that block's end.
    let exits: Vec<(BlockId, BlockId)> = lp
        .linear_order
        .iter()
        .flat_map(|&b| {
            terminator_successors(&func.blocks[&b].terminator)
                .into_iter()
                .filter(|s| !lp.blocks.contains(s))
                .map(move |s| (b, s))
        })
        .collect();
    for (from, to) in exits {
        if !any_dirty {
            continue;
        }
        let vals = values_at_block_end[&from].clone();
        let store_ops = alloc_store_back_ops(func, module_root, &plan.slots, &vals, &slot_dirty);
        // The new edge block forwards the original edge args unchanged.
        let edge_block = func.fresh_block();
        let original_args = edge_args(&func.blocks[&from].terminator, to);
        func.blocks.insert(
            edge_block,
            TirBlock {
                id: edge_block,
                args: vec![],
                ops: store_ops,
                terminator: Terminator::Branch {
                    target: to,
                    args: original_args,
                },
            },
        );
        let from_block = func.blocks.get_mut(&from).expect("exit pred");
        retarget_edge(&mut from_block.terminator, to, edge_block);
    }

    // ---- 5. compensation blocks for dirty CheckExceptions -------------------
    if !compensations.is_empty() {
        let label_to_block: HashMap<i64, BlockId> = func
            .label_id_map
            .iter()
            .map(|(b, l)| (*l, BlockId(*b)))
            .collect();
        let mut next_label = func.label_id_map.values().copied().max().unwrap_or(0) + 1;
        for c in compensations {
            let Some(&handler_block) = label_to_block.get(&c.original_label) else {
                continue; // unresolvable label: leave the op untouched (sound).
            };
            let comp_ops =
                alloc_store_back_ops(func, module_root, &plan.slots, &c.values, &slot_dirty);
            let comp_block = func.fresh_block();
            let fresh_label = next_label;
            next_label += 1;
            func.label_id_map.insert(comp_block.0, fresh_label);
            func.blocks.insert(
                comp_block,
                TirBlock {
                    id: comp_block,
                    args: vec![],
                    ops: comp_ops,
                    terminator: Terminator::Branch {
                        target: handler_block,
                        args: c.original_operands.clone(),
                    },
                },
            );
            let block = func.blocks.get_mut(&c.check_block).expect("check block");
            let op = &mut block.ops[c.check_op_index];
            debug_assert_eq!(op.opcode, OpCode::CheckException);
            op.attrs.insert("value".into(), AttrValue::Int(fresh_label));
            op.operands.clear();
        }
    }
}

/// Build the `ConstStr name` + `ModuleSetAttr` pairs that store the dirty
/// slots back. Fresh ConstStr ops are synthesized per block (the loop's name
/// consts may not dominate the new blocks; string constants are position-free)
/// — which requires `&mut func` for the fresh result ids, so call this BEFORE
/// taking any other borrow of `func.blocks`.
fn alloc_store_back_ops(
    func: &mut TirFunction,
    module_root: ValueId,
    slots: &[String],
    values: &[ValueId],
    dirty: &[bool],
) -> Vec<TirOp> {
    let mut ops = Vec::new();
    for (i, slot) in slots.iter().enumerate() {
        if !dirty[i] {
            continue;
        }
        let name_id = func.fresh_value();
        func.value_types
            .insert(name_id, super::super::types::TirType::Str);
        let mut name_attrs = super::super::ops::AttrDict::new();
        name_attrs.insert("s_value".into(), AttrValue::Str(slot.clone()));
        ops.push(TirOp {
            dialect: super::super::ops::Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![name_id],
            attrs: name_attrs,
            source_span: None,
        });
        ops.push(TirOp {
            dialect: super::super::ops::Dialect::Molt,
            opcode: OpCode::ModuleSetAttr,
            operands: vec![module_root, name_id, values[i]],
            results: vec![],
            attrs: super::super::ops::AttrDict::new(),
            source_span: None,
        });
    }
    ops
}

/// Rewrite every value operand in `term` through `f`.
fn rewrite_terminator_values(term: &mut Terminator, f: &dyn Fn(ValueId) -> ValueId) {
    match term {
        Terminator::Branch { args, .. } => {
            for v in args {
                *v = f(*v);
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            *cond = f(*cond);
            for v in then_args.iter_mut().chain(else_args.iter_mut()) {
                *v = f(*v);
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            *value = f(*value);
            for (_, _, args) in cases.iter_mut() {
                for v in args {
                    *v = f(*v);
                }
            }
            for v in default_args {
                *v = f(*v);
            }
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases.iter_mut() {
                for v in args {
                    *v = f(*v);
                }
            }
            for v in default_args {
                *v = f(*v);
            }
        }
        Terminator::Return { values } => {
            for v in values {
                *v = f(*v);
            }
        }
        Terminator::Unreachable => {}
    }
}

/// The args `term` passes on its edge to `to` (first matching edge).
fn edge_args(term: &Terminator, to: BlockId) -> Vec<ValueId> {
    match term {
        Terminator::Branch { target, args } if *target == to => args.clone(),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == to {
                then_args.clone()
            } else if *else_block == to {
                else_args.clone()
            } else {
                vec![]
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => cases
            .iter()
            .find(|(_, b, _)| *b == to)
            .map(|(_, _, a)| a.clone())
            .unwrap_or_else(|| {
                if *default == to {
                    default_args.clone()
                } else {
                    vec![]
                }
            }),
        _ => vec![],
    }
}

/// Retarget every edge in `term` from `old` to `new`, clearing the edge args
/// (the new edge block forwards the originals itself).
fn retarget_edge(term: &mut Terminator, old: BlockId, new: BlockId) {
    match term {
        Terminator::Branch { target, args } if *target == old => {
            *target = new;
            args.clear();
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == old {
                *then_block = new;
                then_args.clear();
            }
            if *else_block == old {
                *else_block = new;
                else_args.clear();
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, b, args) in cases.iter_mut() {
                if *b == old {
                    *b = new;
                    args.clear();
                }
            }
            if *default == old {
                *default = new;
                default_args.clear();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::blocks::{Terminator, TirBlock};
    use super::super::super::function::{TirFunction, TirModule};
    use super::super::super::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use super::super::super::types::TirType;
    use super::super::super::values::ValueId;
    use super::*;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn const_str(func: &mut TirFunction, s: &str) -> (TirOp, ValueId) {
        let r = func.fresh_value();
        let mut o = op(OpCode::ConstStr, vec![], vec![r]);
        o.attrs.insert("s_value".into(), AttrValue::Str(s.into()));
        (o, r)
    }

    fn const_int(func: &mut TirFunction, v: i64) -> (TirOp, ValueId) {
        let r = func.fresh_value();
        let mut o = op(OpCode::ConstInt, vec![], vec![r]);
        o.attrs.insert("value".into(), AttrValue::Int(v));
        func.value_types.insert(r, TirType::I64);
        (o, r)
    }

    /// The bench_sum chunk shape: preheader sets total/i/N as module attrs,
    /// a jump-shaped while loop reads/writes them per iteration with a
    /// CheckException (handler label 7 → block 4), exit reads total.
    fn module_loop_func() -> TirFunction {
        let mut f = TirFunction::new("chunk".into(), vec![TirType::DynBox], TirType::DynBox);
        let m = ValueId(0);
        let header = f.fresh_block();
        let body = f.fresh_block();
        let exit = f.fresh_block();
        let handler = f.fresh_block();

        // Preheader (entry): total = 0; i = 0; N = 100.
        let (ct0, ct0v) = const_str(&mut f, "total");
        let (zero_op, zero) = const_int(&mut f, 0);
        let (ci0, ci0v) = const_str(&mut f, "i");
        let (cn0, cn0v) = const_str(&mut f, "N");
        let (n_op, nval) = const_int(&mut f, 100);
        {
            let e = f.entry_block;
            let entry = f.blocks.get_mut(&e).unwrap();
            entry.ops = vec![
                ct0,
                zero_op,
                op(OpCode::ModuleSetAttr, vec![m, ct0v, zero], vec![]),
                ci0,
                op(OpCode::ModuleSetAttr, vec![m, ci0v, zero], vec![]),
                cn0,
                n_op,
                op(OpCode::ModuleSetAttr, vec![m, cn0v, nval], vec![]),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![],
            };
        }

        // Header: vi = get i; vn = get N; cond = Lt(vi, vn); CondBranch.
        let (ci1, ci1v) = const_str(&mut f, "i");
        let vi = f.fresh_value();
        let (cn1, cn1v) = const_str(&mut f, "N");
        let vn = f.fresh_value();
        let cond = f.fresh_value();
        f.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![
                    ci1,
                    op(OpCode::ModuleGetAttr, vec![m, ci1v], vec![vi]),
                    cn1,
                    op(OpCode::ModuleGetAttr, vec![m, cn1v], vec![vn]),
                    op(OpCode::Lt, vec![vi, vn], vec![cond]),
                ],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // Body: vt = get total; sum = Add(vt, vi); CheckException(label 7);
        // set total = sum; ni = Add(vi, 1); set i = ni; Branch header.
        let (ct1, ct1v) = const_str(&mut f, "total");
        let vt = f.fresh_value();
        let sum = f.fresh_value();
        let (ct2, ct2v) = const_str(&mut f, "total");
        let (one_op, one) = const_int(&mut f, 1);
        let ni = f.fresh_value();
        let (ci2, ci2v) = const_str(&mut f, "i");
        let mut check = op(OpCode::CheckException, vec![], vec![]);
        check.attrs.insert("value".into(), AttrValue::Int(7));
        f.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    ct1,
                    op(OpCode::ModuleGetAttr, vec![m, ct1v], vec![vt]),
                    op(OpCode::Add, vec![vt, vi], vec![sum]),
                    check,
                    ct2,
                    op(OpCode::ModuleSetAttr, vec![m, ct2v, sum], vec![]),
                    one_op,
                    op(OpCode::Add, vec![vi, one], vec![ni]),
                    ci2,
                    op(OpCode::ModuleSetAttr, vec![m, ci2v, ni], vec![]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        // Exit: r = get total; return r.
        let (ct3, ct3v) = const_str(&mut f, "total");
        let r = f.fresh_value();
        f.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![ct3, op(OpCode::ModuleGetAttr, vec![m, ct3v], vec![r])],
                terminator: Terminator::Return { values: vec![r] },
            },
        );

        // Handler (label 7): bare return (the function exception exit).
        f.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        f.label_id_map.insert(handler.0, 7);
        f
    }

    fn count_module_ops_in(func: &TirFunction, blocks: &[BlockId]) -> usize {
        blocks
            .iter()
            .map(|b| {
                func.blocks[b]
                    .ops
                    .iter()
                    .filter(|o| matches!(o.opcode, OpCode::ModuleGetAttr | OpCode::ModuleSetAttr))
                    .count()
            })
            .sum()
    }

    #[test]
    fn promotes_bench_sum_shaped_loop() {
        let f = module_loop_func();
        let header = BlockId(1);
        let body = BlockId(2);
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![f],
        };
        let (stats, changed) = run_module_slot_promotion(&mut module);

        assert_eq!(changed, vec!["chunk".to_string()], "function promoted");
        assert_eq!(stats.slots_promoted, 3, "total, i, N promoted");
        assert_eq!(stats.ops_eliminated, 5, "3 gets + 2 sets eliminated");
        let f = &module.functions[0];
        assert_eq!(
            count_module_ops_in(f, &[header, body]),
            0,
            "no module-attr traffic left inside the loop"
        );
        assert_eq!(f.blocks[&header].args.len(), 3, "carried phis added");
        // The merged function is structurally valid SSA.
        crate::tir::verify::verify_function(f)
            .unwrap_or_else(|e| panic!("promoted fn invalid: {e:?}"));
        // A compensation block exists: some block (≠ original handler) carries
        // ModuleSetAttr ops AND branches to the handler block (BlockId(4)).
        let handler = BlockId(4);
        let comp_exists = f.blocks.values().any(|b| {
            b.id != handler
                && b.ops.iter().any(|o| o.opcode == OpCode::ModuleSetAttr)
                && matches!(
                    &b.terminator,
                    Terminator::Branch { target, .. } if *target == handler
                )
        });
        assert!(comp_exists, "CheckException compensation block present");
        // The exit path stores the dirty slots back (an edge block with sets
        // branching to the original exit block).
        let exit = BlockId(3);
        let exit_store_exists = f.blocks.values().any(|b| {
            b.id != exit
                && b.ops.iter().any(|o| o.opcode == OpCode::ModuleSetAttr)
                && matches!(
                    &b.terminator,
                    Terminator::Branch { target, .. } if *target == exit
                )
        });
        assert!(exit_store_exists, "exit-edge store-back block present");
    }

    #[test]
    fn threading_import_disables_promotion_module_wide() {
        let f = module_loop_func();
        // A second function importing `threading` — a concurrent observer of
        // module globals may then exist.
        let mut g = TirFunction::new("spawner".into(), vec![], TirType::None);
        let imp_res = g.fresh_value();
        let mut imp = op(OpCode::Import, vec![], vec![imp_res]);
        imp.attrs
            .insert("s_value".into(), AttrValue::Str("threading".into()));
        {
            let e = g.entry_block;
            let entry = g.blocks.get_mut(&e).unwrap();
            entry.ops = vec![imp];
            entry.terminator = Terminator::Return { values: vec![] };
        }
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![f, g],
        };
        let (stats, changed) = run_module_slot_promotion(&mut module);
        assert!(
            changed.is_empty(),
            "threading import => module-wide refusal"
        );
        assert_eq!(stats.slots_promoted, 0);
    }

    #[test]
    fn thread_intrinsic_name_string_alone_does_not_refuse() {
        // The always-linked stdlib wrapper bodies carry `molt_thread_*` NAME
        // STRINGS (annotations, require_intrinsic args). A mere string must NOT
        // refuse promotion — only an Import of threading/_thread or a direct
        // molt_thread_* CALL does. (The over-broad string heuristic refused
        // every program: the needs_inlining trap, round two.)
        let f = module_loop_func();
        let mut g = TirFunction::new("wrapper".into(), vec![], TirType::None);
        let (marker, _) = const_str(&mut g, "molt_thread_spawn");
        {
            let e = g.entry_block;
            let entry = g.blocks.get_mut(&e).unwrap();
            entry.ops = vec![marker];
            entry.terminator = Terminator::Return { values: vec![] };
        }
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![f, g],
        };
        let (stats, changed) = run_module_slot_promotion(&mut module);
        assert_eq!(changed, vec!["chunk".to_string()], "string alone is benign");
        assert_eq!(stats.slots_promoted, 3);
    }

    #[test]
    fn call_in_loop_refuses_promotion() {
        let mut f = module_loop_func();
        // Insert an opaque call into the loop body — GenericHeap aliases
        // ModuleDict, so the loop must be refused.
        let body = BlockId(2);
        let mut call = op(OpCode::Call, vec![], vec![]);
        call.attrs
            .insert("s_value".into(), AttrValue::Str("opaque".into()));
        f.blocks.get_mut(&body).unwrap().ops.insert(0, call);
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![f],
        };
        let (stats, changed) = run_module_slot_promotion(&mut module);
        assert!(changed.is_empty(), "opaque call in loop => refusal");
        assert_eq!(stats.slots_promoted, 0);
    }

    #[test]
    fn typed_field_store_in_loop_does_not_refuse_promotion() {
        // A `guarded_field_set` (TypedField region) writes a class instance's
        // own fixed-layout slot — it can NEVER mutate a module-dict slot, so it
        // must NOT disqualify promotion of the module-dict slots in the loop.
        // (Pre-S5-1.5 this op was GenericHeap and aliased ModuleDict, wrongly
        // refusing the loop.) Object identity for the field op is irrelevant; we
        // use a fresh value as the instance.
        let mut f = module_loop_func();
        let body = BlockId(2);
        let inst = f.fresh_value();
        let val = f.fresh_value();
        // guarded_field_set: operands [obj, class_bits, version, val]; the alias
        // oracle only reads operand[0]=obj, offset (`value`), class (`_class`).
        let cbits = f.fresh_value();
        let ver = f.fresh_value();
        let mut fset = op(OpCode::StoreAttr, vec![inst, cbits, ver, val], vec![]);
        fset.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("guarded_field_set".into()),
        );
        fset.attrs.insert("value".into(), AttrValue::Int(0));
        fset.attrs
            .insert("_class".into(), AttrValue::Str("Counter".into()));
        f.blocks.get_mut(&body).unwrap().ops.insert(0, fset);
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![f],
        };
        let (stats, changed) = run_module_slot_promotion(&mut module);
        assert_eq!(
            changed,
            vec!["chunk".to_string()],
            "a TypedField store does not alias ModuleDict; promotion proceeds"
        );
        assert_eq!(stats.slots_promoted, 3);
    }
}
