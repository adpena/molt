//! Pass manager for the TIR optimization pipeline (Tier-0 substrate **S1**).
//!
//! Replaces the former monolithic `run_pipeline` linear sequence with a
//! uniform [`TirPass`] abstraction threaded through a per-function
//! [`AnalysisManager`](crate::tir::analysis::AnalysisManager). Each pass
//! declares a [`Mutates`] class; after any pass that may change the CFG, the
//! [`PassManager`] invalidates every CFG-sensitive analysis so the next pass
//! recomputes against the new shape.
//!
//! The default pipeline ([`build_default_pipeline`]) preserves the EXACT
//! 26-pass order, the snapshot/restore-on-zero-delta behavior, and the
//! post-pipeline `verify_function` of the legacy `run_pipeline`. `run_pipeline`
//! is now a thin entry that builds the default pipeline and runs it — the real
//! API, not a shim.
//!
//! ## Invalidation soundness (the critical contract)
//!
//! A stale cached dominator tree after a CFG mutation is a **miscompile**. The
//! design is FAIL-CLOSED:
//!
//! * [`Mutates`] defaults to `Cfg` (the most conservative class) — a pass that
//!   forgets to declare its mutation class over-invalidates (a redundant
//!   recompute), never under-invalidates.
//! * After every pass whose class is `Mutates::Cfg`, the manager calls
//!   [`AnalysisManager::invalidate_cfg`](crate::tir::analysis::AnalysisManager::invalidate_cfg),
//!   dropping all CFG-sensitive analyses atomically.
//! * `Mutates::OpsOnly` (rewrites op operands/attrs but never changes the block
//!   set, edges, or terminators) and `Mutates::ReadOnly` (pure analysis/marking
//!   passes) do NOT invalidate the CFG analyses — their results stay valid.
//!
//! ## Debug self-check: `MOLT_VERIFY_ANALYSIS=1`
//!
//! When the env var is set, after EVERY pass the manager recomputes each
//! CFG-sensitive analysis from a fresh manager and asserts it equals the cached
//! value. This catches the soundness-fatal case where a pass mutates the CFG
//! but is mis-declared `OpsOnly`/`ReadOnly`: the cached analysis would diverge
//! from a fresh recompute and the assert fires immediately, pinning the
//! offending pass. (Off by default — it doubles analysis cost.)

use super::analysis::{
    AnalysisManager, DefMap, DomChildren, ExecReachable, ImmediateDoms, LoopForest, PredMap,
    StrictReachable,
};
use super::function::TirFunction;
use super::passes::{self, PassStats};
use super::target_info::TargetInfo;

/// How a pass may mutate the function — drives analysis invalidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutates {
    /// May change the block set, CFG edges, or terminators. The PassManager
    /// invalidates all CFG-sensitive analyses afterward. This is the safe
    /// default for any pass whose mutation profile is uncertain.
    Cfg,
    /// Rewrites op operands/results/attrs in place — and may add/remove ops
    /// *within* a block — but never changes the block set, CFG edges, or
    /// terminators. The dominator/pred/reachability/loop-forest analyses stay
    /// valid; the ops-sensitive [`DefMap`](crate::tir::analysis::DefMap) is
    /// invalidated.
    ///
    /// CRITICAL INVARIANT: an `OpsOnly` pass MUST NOT add or remove an op that
    /// carries an exception edge (`CheckException`/`TryStart`/`TryEnd`/
    /// `StateBlockStart`/`StateBlockEnd`) — those edges are part of the
    /// exception-augmented CFG the dominator analyses are built over. A pass
    /// that touches them, removes a whole block, redirects an edge, or rewrites
    /// a terminator is `Cfg`, not `OpsOnly`. The default-pipeline `OpsOnly`
    /// passes are verified to honor this (they only remove
    /// IncRef/DecRef/arithmetic/copy ops).
    OpsOnly,
    /// Pure analysis or attribute marking — does not change executable IR at
    /// all (e.g. BCE adds a `bce_safe` attr; reuse marks metadata).
    ReadOnly,
}

/// A uniform optimization pass over a [`TirFunction`].
pub trait TirPass {
    /// Stable pass name (matches the legacy `PassStats.name`).
    fn name(&self) -> &'static str;

    /// What this pass may mutate — drives CFG-analysis invalidation. Default is
    /// the conservative `Cfg` (fail-closed: over-invalidate, never under).
    fn mutation_class(&self) -> Mutates {
        Mutates::Cfg
    }

    /// Run the pass, returning its stats. The [`AnalysisManager`] provides
    /// cached dominators / pred map / reachability / loop forest / def map; the
    /// [`TargetInfo`] is the unified cost model (Tier-0 S2) the pass consults
    /// for every profitability decision instead of a hardcoded constant.
    fn run(
        &self,
        func: &mut TirFunction,
        am: &mut AnalysisManager,
        tti: &TargetInfo,
    ) -> PassStats;
}

/// Adapter wrapping a legacy `fn(&mut TirFunction) -> PassStats` (or a closure
/// taking the AnalysisManager / TargetInfo) as a [`TirPass`] with an explicit
/// mutation class. The function pointer form keeps the pipeline table compact
/// and branch-predictable.
struct FnPass {
    name: &'static str,
    mutates: Mutates,
    run: fn(&mut TirFunction, &mut AnalysisManager, &TargetInfo) -> PassStats,
}

impl TirPass for FnPass {
    fn name(&self) -> &'static str {
        self.name
    }
    fn mutation_class(&self) -> Mutates {
        self.mutates
    }
    fn run(
        &self,
        func: &mut TirFunction,
        am: &mut AnalysisManager,
        tti: &TargetInfo,
    ) -> PassStats {
        (self.run)(func, am, tti)
    }
}

/// Construct an [`FnPass`] boxed as `dyn TirPass`.
fn pass(
    name: &'static str,
    mutates: Mutates,
    run: fn(&mut TirFunction, &mut AnalysisManager, &TargetInfo) -> PassStats,
) -> Box<dyn TirPass> {
    Box::new(FnPass {
        name,
        mutates,
        run,
    })
}

/// The TIR pass pipeline: an ordered list of passes, the unified cost model
/// (Tier-0 S2), plus the analysis manager threading and invalidation logic.
pub struct PassManager {
    passes: Vec<Box<dyn TirPass>>,
    /// The cost model threaded to every pass's `run`. Owned by the manager so a
    /// single per-(target, profile) instance drives every profitability
    /// decision in the pipeline.
    target_info: TargetInfo,
}

impl PassManager {
    pub fn new(passes: Vec<Box<dyn TirPass>>, target_info: TargetInfo) -> Self {
        Self {
            passes,
            target_info,
        }
    }

    /// The cost model this manager threads to its passes.
    pub fn target_info(&self) -> &TargetInfo {
        &self.target_info
    }

    /// Pass names in pipeline order (test/diagnostic aid).
    pub fn pass_names(&self) -> Vec<&'static str> {
        self.passes.iter().map(|p| p.name()).collect()
    }

    /// Run every pass on `func`, threading a fresh [`AnalysisManager`].
    ///
    /// Mirrors the legacy `run_pipeline` exactly: snapshot before mutation,
    /// run all passes recording stats, restore the snapshot on zero net delta,
    /// dump pre/post TIR when requested, and panic if post-pipeline
    /// verification fails.
    pub fn run(&self, func: &mut TirFunction) -> Vec<PassStats> {
        let verify_analysis = std::env::var("MOLT_VERIFY_ANALYSIS").as_deref() == Ok("1");
        self.run_inner(func, verify_analysis)
    }

    /// Pipeline body with the per-pass analysis self-check explicitly
    /// controlled (rather than read from the process-global env), so tests can
    /// force it on deterministically without racing other parallel tests.
    fn run_inner(&self, func: &mut TirFunction, verify_analysis: bool) -> Vec<PassStats> {
        // Snapshot BEFORE any mutation so unchanged pipelines lower the
        // original IR structurally without pass-induced metadata drift.
        let snapshot = func.clone();

        let mut stats = Vec::with_capacity(passes::PIPELINE_PASS_CAPACITY_HINT);

        let has_loop_role = !func.loop_roles.is_empty();
        let dump_tir =
            std::env::var("MOLT_DUMP_IR").is_ok() || std::env::var("TIR_DUMP").is_ok();
        if has_loop_role && dump_tir {
            dump_tir_artifact(func, "pre", &[]);
        }

        let mut am = AnalysisManager::new();

        for p in &self.passes {
            let mut stat = p.run(func, &mut am, &self.target_info);
            stat.name = p.name();

            // Invalidate analyses according to the pass's mutation class.
            // FAIL-CLOSED: `Cfg` drops every CFG- and ops-sensitive analysis;
            // `OpsOnly` drops ops-sensitive analyses (def map) while keeping the
            // CFG-structure analyses (op rewrites don't change edges, and the
            // OpsOnly passes provably never add/remove exception-edge-bearing
            // ops — see `build_default_pipeline`); `ReadOnly` keeps everything.
            match p.mutation_class() {
                Mutates::Cfg => am.invalidate_cfg(),
                Mutates::OpsOnly => am.invalidate_ops(),
                Mutates::ReadOnly => {}
            }

            // Debug soundness self-check: every still-cached CFG-sensitive
            // analysis must equal a fresh recompute. A pass that mutated the
            // CFG but declared OpsOnly/ReadOnly would diverge here.
            if verify_analysis {
                assert_analyses_fresh(func, &mut am, p.name());
            }

            stats.push(stat);
        }

        let total_changes: usize = stats
            .iter()
            .map(|s| s.values_changed + s.ops_removed + s.ops_added)
            .sum();
        if total_changes == 0 {
            *func = snapshot.clone();
        }

        if has_loop_role && dump_tir {
            dump_tir_artifact(func, "post", &stats);
        }

        if let Err(errors) = super::verify::verify_function(func) {
            panic!(
                "[TIR] verification failed after optimization of '{}': {:?}",
                func.name, errors
            );
        }

        if std::env::var("TIR_OPT_STATS").as_deref() == Ok("1") {
            for s in &stats {
                eprintln!(
                    "[TIR] {}: {} values changed, {} ops removed, {} ops added",
                    s.name, s.values_changed, s.ops_removed, s.ops_added
                );
            }
        }

        stats
    }
}

/// Build the default 26-pass pipeline in the canonical order. The ordering and
/// per-pass behavior are byte-for-byte identical to the legacy `run_pipeline`;
/// only the dispatch and analysis-caching mechanism changed.
///
/// Phase ordering rationale (unchanged from the legacy pipeline):
/// * **Lowering** devirtualizes iterators/ranges and unrolls fixed-trip loops,
///   exposing concrete control flow to later phases.
/// * **Canonicalization** runs twice (instcombine pattern): once pre-type, once
///   post-unboxing.
/// * **Redundancy** (GVN, LICM) runs after canonicalization and type settling.
/// * **Memory** (escape, refcount, reuse, dead-store) runs after redundancy.
/// * **Value** specialization runs late so it sees the final type lattice.
/// * **Cleanup** (check-exception elim, copy-prop, DCE) runs last.
///
/// Mutation classes (each verified against the pass body):
/// * `Cfg` — may add/remove blocks, redirect edges, or rewrite terminators:
///   range_devirt, iter_devirt, loop_unroll, block_versioning, licm (preheader
///   insertion), type_guard_hoist (moves ops across blocks), sccp (branch
///   fold), branchless_count (folds a CondBranch cond-block to a Branch and
///   removes the then/else blocks), check_exception_elim (drops blocks), dce
///   (block removal).
/// * `OpsOnly` — rewrites/removes ops within blocks, never an exception-edge
///   op or terminator: canonicalize (×2), unboxing, gvn, refcount_elim,
///   escape_analysis (removes IncRef/DecRef, rewrites ObjectNewBound),
///   dead_store_elim, strength_reduction, fast_math, copy_prop,
///   tuple_scalarize.
/// * `ReadOnly` — only marks attrs/metadata, no executable-IR change: bce
///   (`bce_safe` attr), reuse_analysis, vectorize, polyhedral.
pub fn build_default_pipeline(target_info: TargetInfo) -> PassManager {
    use Mutates::*;
    // The `am`/`tti`-ignoring adapters wrap legacy passes that consume neither
    // the analysis manager nor the cost model; the analysis-consuming passes
    // take `am` and the cost-model-consuming passes take `tti` through their
    // migrated `run` signatures.
    let passes: Vec<Box<dyn TirPass>> = vec![
        // ── Lowering ────────────────────────────────────────────────
        pass("range_devirt", Cfg, |f, _am, _tti| {
            passes::range_devirt::run(f)
        }),
        pass("iter_devirt", Cfg, |f, _am, _tti| passes::iter_devirt::run(f)),
        pass("tuple_scalarize", OpsOnly, |f, _am, _tti| {
            passes::deforestation::run_tuple_scalarize(f)
        }),
        pass("loop_unroll", Cfg, |f, _am, tti| {
            passes::loop_unroll::run(f, tti)
        }),
        // ── Canonicalization (phase 1) ──────────────────────────────
        pass("canonicalize", OpsOnly, |f, _am, _tti| {
            passes::canonicalize::run(f)
        }),
        // ── Type-directed optimization ──────────────────────────────
        pass("unboxing", OpsOnly, |f, _am, _tti| passes::unboxing::run(f)),
        pass("block_versioning", Cfg, |f, am, _tti| {
            passes::block_versioning::run(f, am)
        }),
        // ── Canonicalization (phase 2) ──────────────────────────────
        pass("canonicalize_post", OpsOnly, |f, _am, _tti| {
            passes::canonicalize::run(f)
        }),
        // ── Global redundancy elimination ───────────────────────────
        pass("gvn", OpsOnly, |f, am, _tti| passes::gvn::run(f, am)),
        pass("licm", Cfg, |f, am, _tti| passes::licm::run(f, am)),
        // ── Memory optimization ─────────────────────────────────────
        pass("escape_analysis", OpsOnly, |f, _am, _tti| {
            passes::escape_analysis::run(f)
        }),
        pass("refcount_elim", OpsOnly, |f, am, _tti| {
            passes::refcount_elim::run(f, am)
        }),
        pass("reuse_analysis", ReadOnly, |f, am, _tti| {
            passes::reuse_analysis::run(f, am)
        }),
        pass("dead_store_elim", OpsOnly, |f, am, _tti| {
            passes::dead_store_elim::run(f, am)
        }),
        // MemGVN consumes MemorySSA (built on the class-aware TypedField alias
        // regions) to forward stores into proven-pure typed-slot loads and dedup
        // redundant loads. Placed AFTER dead_store_elim so it sees the final set
        // of live stores, and its replacement IncRef is final (refcount_elim has
        // already run). OpsOnly: replaces a load with IncRef+Copy in place.
        pass("mem_gvn", OpsOnly, |f, am, _tti| passes::mem_gvn::run(f, am)),
        // ── Value optimization ──────────────────────────────────────
        pass("type_guard_hoist", Cfg, |f, am, _tti| {
            passes::type_guard_hoist::run(f, am)
        }),
        pass("sccp", Cfg, |f, _am, _tti| passes::sccp::run(f)),
        pass("strength_reduction", OpsOnly, |f, _am, _tti| {
            passes::strength_reduction::run(f)
        }),
        pass("fast_math", OpsOnly, |f, _am, _tti| passes::fast_math::run(f)),
        // branchless_count rewrites a `CondBranch` cond-block into a `Branch`
        // and removes the then/else blocks → CFG mutation. Gated by the cost
        // model's branchless-rewrite profitability query.
        pass("branchless_count", Cfg, |f, _am, tti| {
            passes::branchless_count::run(f, tti)
        }),
        pass("bce", ReadOnly, |f, am, _tti| passes::bce::run(f, am)),
        pass("vectorize", ReadOnly, |f, _am, tti| {
            passes::vectorize::run(f, tti)
        }),
        pass("polyhedral", ReadOnly, |f, _am, tti| {
            passes::polyhedral::run(f, tti)
        }),
        // ── Cleanup ─────────────────────────────────────────────────
        pass("check_exception_elim", Cfg, |f, _am, _tti| {
            passes::check_exception_elim::run(f)
        }),
        // overflow_peel runs AFTER check_exception_elim (the loop body only
        // becomes the pure Copies+Adds shape its recognizer requires once the
        // per-op CheckExceptions are eliminated; a body that retains one
        // correctly refuses via the purity scan) and BEFORE copy_prop/dce
        // (which are shape-preserving cleanups).
        pass("overflow_peel", Cfg, |f, am, _tti| {
            passes::overflow_peel::run(f, am)
        }),
        pass("copy_prop", OpsOnly, |f, _am, _tti| passes::copy_prop::run(f)),
        pass("dce", Cfg, |f, _am, _tti| passes::dce::run(f)),
    ];
    PassManager::new(passes, target_info)
}

// ---------------------------------------------------------------------------
// Debug self-check
// ---------------------------------------------------------------------------

/// Recompute every CFG-sensitive analysis from a fresh manager and assert it
/// equals what `am` currently has cached. Any divergence means a pass mutated
/// the CFG without declaring `Mutates::Cfg`.
fn assert_analyses_fresh(func: &TirFunction, am: &mut AnalysisManager, after_pass: &str) {
    use super::analysis::AnalysisId;

    let mut fresh = AnalysisManager::new();
    for id in AnalysisId::ALL {
        if !am.is_cached(id) {
            continue;
        }
        macro_rules! check {
            ($A:ty) => {{
                let cached = am.get::<$A>(func).clone();
                let recomputed = fresh.get::<$A>(func).clone();
                assert!(
                    cached == recomputed,
                    "[MOLT_VERIFY_ANALYSIS] stale {:?} after pass '{}' in '{}': \
                     cached analysis diverges from fresh recompute — the pass \
                     mutated the CFG but is not declared Mutates::Cfg",
                    id,
                    after_pass,
                    func.name,
                );
            }};
        }
        match id {
            AnalysisId::PredMap => check!(PredMap),
            AnalysisId::ImmediateDoms => check!(ImmediateDoms),
            AnalysisId::DomChildren => check!(DomChildren),
            AnalysisId::ExecReachable => check!(ExecReachable),
            AnalysisId::StrictReachable => check!(StrictReachable),
            AnalysisId::LoopForest => check!(LoopForest),
            AnalysisId::DefMap => check!(DefMap),
            AnalysisId::ScalarEvolution => {
                check!(super::passes::scev::ScalarEvolution)
            }
            AnalysisId::ValueRange => check!(super::passes::value_range::ValueRange),
            AnalysisId::AliasAnalysis => check!(super::passes::alias_analysis::AliasAnalysis),
            AnalysisId::MemorySSA => check!(super::passes::memory_ssa::MemorySSA),
        }
    }
}

// ---------------------------------------------------------------------------
// TIR dump artifact (moved verbatim from the legacy run_pipeline)
// ---------------------------------------------------------------------------

fn dump_tir_artifact(func: &TirFunction, phase: &str, stats: &[PassStats]) {
    use super::blocks::Terminator;

    let sanitized: String = func
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let label = if phase == "pre" { "PRE-OPT" } else { "POST-OPT" };
    let mut dump = format!(
        "// {} TIR: {} (loop_roles={:?})\n",
        label, func.name, func.loop_roles
    );
    if phase == "pre" {
        dump.push_str(&format!("// blocks: {}\n", func.blocks.len()));
    } else {
        dump.push_str(&format!(
            "// stats: {:?}\n",
            stats
                .iter()
                .map(|s| (s.name, s.values_changed, s.ops_removed, s.ops_added))
                .collect::<Vec<_>>()
        ));
    }

    let mut bids: Vec<_> = func.blocks.keys().copied().collect();
    bids.sort_by_key(|b| b.0);
    for bid in &bids {
        let block = &func.blocks[bid];
        dump.push_str(&format!(
            "\nblock {} (args={}, ops={}):\n",
            bid.0,
            block.args.len(),
            block.ops.len()
        ));
        for op in &block.ops {
            dump.push_str(&format!(
                "  {:?} operands={:?} results={:?}\n",
                op.opcode, op.operands, op.results
            ));
        }
        if phase == "pre" {
            dump.push_str(&format!(
                "  TERM: {:?}\n",
                std::mem::discriminant(&block.terminator)
            ));
            match &block.terminator {
                Terminator::Branch { target, args } => {
                    dump.push_str(&format!("    → block {} args={:?}\n", target.0, args))
                }
                Terminator::CondBranch {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => dump.push_str(&format!(
                    "    cond={:?} then={} else={}\n",
                    cond, then_block.0, else_block.0
                )),
                Terminator::Return { values } => {
                    dump.push_str(&format!("    return {} values\n", values.len()))
                }
                _ => {}
            }
        } else {
            match &block.terminator {
                Terminator::Branch { target, args } => dump.push_str(&format!(
                    "  TERM: Branch → block {} args={:?}\n",
                    target.0, args
                )),
                Terminator::CondBranch {
                    cond,
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                } => dump.push_str(&format!(
                    "  TERM: CondBranch cond={:?} then={} args={:?} else={} args={:?}\n",
                    cond, then_block.0, then_args, else_block.0, else_args
                )),
                Terminator::Return { values } => {
                    dump.push_str(&format!("  TERM: Return {} values\n", values.len()))
                }
                _ => dump.push_str("  TERM: other\n"),
            }
        }
    }

    let _ = crate::debug_artifacts::write_debug_artifact(
        format!("tir/{}_{}.txt", sanitized, phase),
        dump,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    /// The default pipeline must preserve the EXACT canonical pass order (26
    /// `run` invocations — canonicalize runs twice). Any reorder/insert/drop is
    /// a behavior change and must update this list deliberately.
    #[test]
    fn default_pipeline_preserves_canonical_pass_order() {
        let pm = build_default_pipeline(TargetInfo::native_release_fast());
        assert_eq!(
            pm.pass_names(),
            vec![
                "range_devirt",
                "iter_devirt",
                "tuple_scalarize",
                "loop_unroll",
                "canonicalize",
                "unboxing",
                "block_versioning",
                "canonicalize_post",
                "gvn",
                "licm",
                "escape_analysis",
                "refcount_elim",
                "reuse_analysis",
                "dead_store_elim",
                "mem_gvn",
                "type_guard_hoist",
                "sccp",
                "strength_reduction",
                "fast_math",
                "branchless_count",
                "bce",
                "vectorize",
                "polyhedral",
                "check_exception_elim",
                "overflow_peel",
                "copy_prop",
                "dce",
            ],
        );
    }

    /// Every pass that may change the CFG must be declared `Mutates::Cfg`; the
    /// CFG-mutating passes are enumerated so a future miscategorization (e.g.
    /// declaring `branchless_count` `OpsOnly` again) trips this test.
    #[test]
    fn cfg_mutating_passes_are_declared_cfg() {
        let pm = build_default_pipeline(TargetInfo::native_release_fast());
        let cfg_passes: Vec<&'static str> = pm
            .passes
            .iter()
            .filter(|p| p.mutation_class() == Mutates::Cfg)
            .map(|p| p.name())
            .collect();
        assert_eq!(
            cfg_passes,
            vec![
                "range_devirt",
                "iter_devirt",
                "loop_unroll",
                "block_versioning",
                "licm",
                "type_guard_hoist",
                "sccp",
                "branchless_count",
                "check_exception_elim",
                "overflow_peel",
                "dce",
            ],
        );
    }

    /// End-to-end: a loop-bearing function runs the full pipeline through the
    /// PassManager (exercising analysis caching + CFG invalidation across all
    /// passes) and still verifies. The debug self-check is forced on (via
    /// `run_inner`, not the global env, to avoid racing parallel tests) so a
    /// stale-cache misclassification would panic here.
    #[test]
    fn full_pipeline_on_loop_function_with_verify_guard() {
        // while-style loop: entry → header; header cond → body / exit;
        // body → header (back-edge).
        let mut func = TirFunction::new("loopfn".into(), vec![], TirType::None);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let cond = func.fresh_value();
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Branch { target: header, args: vec![] };
        func.blocks.insert(header, TirBlock {
            id: header,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstBool,
                operands: vec![],
                results: vec![cond],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        });
        func.blocks.insert(body, TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch { target: header, args: vec![] },
        });
        func.blocks.insert(exit, TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        });
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let pm = build_default_pipeline(TargetInfo::native_release_fast());
        // Force the per-pass analysis self-check on for this run.
        let stats = pm.run_inner(&mut func, true);
        // All 26 pass invocations ran.
        assert_eq!(stats.len(), 26);
    }
}
