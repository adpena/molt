//! Pass manager for the TIR optimization pipeline (Tier-0 substrate **S1**).
//!
//! Replaces the former monolithic `run_pipeline` linear sequence with a
//! uniform [`TirPass`] abstraction threaded through a per-function
//! [`AnalysisManager`](crate::tir::analysis::AnalysisManager). Each pass
//! declares a [`Mutates`] class; after any pass that may change the CFG, the
//! [`PassManager`] invalidates every CFG-sensitive analysis so the next pass
//! recomputes against the new shape.
//!
//! The default pipeline ([`build_default_pipeline`]) preserves the exact
//! optimization-pass order, the snapshot/restore-on-zero-delta behavior, and the
//! post-pipeline `verify_function` of the legacy `run_pipeline`. RC
//! drop-insertion now runs in a separate terminal phase
//! ([`build_drop_pipeline`]) after module transforms.
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
use super::target_info::{TargetInfo, TargetKind};

fn trace_func_enabled(name: &str) -> bool {
    std::env::var("MOLT_TIR_TRACE_FUNC")
        .ok()
        .is_some_and(|filter| filter == "1" || name.contains(&filter))
}

fn function_op_count(func: &TirFunction) -> usize {
    func.blocks.values().map(|block| block.ops.len()).sum()
}

/// Whether the RC drop-insertion pass (design 20) is sound to run for a given
/// backend. See the activation note in [`build_default_pipeline`] for the full
/// rationale; the short version is: a backend qualifies iff it consumes the
/// inserted `DecRef`/`IncRef` ops by SSA-value identity (or a 1:1 value↔slot
/// mapping) AND runs no competing automatic temp-refcount mechanism that would
/// double-release the same values.
///
/// * `Llvm`, `Wasm`, `NativeCranelift`, `Luau` — qualify. LLVM is value-keyed;
///   WASM has a 1:1 name↔NaN-boxed-local mapping and no tracked-var auto-RC;
///   native suppresses its automatic temp-RC substrate only on full-function
///   `drop_inserted` functions; the narrower
///   `exception_region_drops_inserted` fact protects pre-bail exception releases
///   without disabling legacy native RC. Luau is GC-managed and lowers the ops to
///   nothing. The native loop-phi raw/heap representation blocker is pinned by
///   `reachable_heap_incoming_poisons_raw_loop_phi`, so the shared TIR drop
///   plane is now the native RC authority as well.
const fn target_uses_tir_drop_insertion(target: TargetKind) -> bool {
    match target {
        TargetKind::Llvm | TargetKind::Wasm | TargetKind::NativeCranelift | TargetKind::Luau => {
            true
        }
    }
}

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
    /// carries an exception transfer edge (`CheckException`/`TryStart`) or
    /// mutates the exception-region stack (`TryEnd`/`StateBlockStart`/
    /// `StateBlockEnd`). Transfer edges are part of the exception-augmented CFG
    /// the dominator analyses are built over; stack markers are lifetime
    /// boundaries even when they are not CFG edges. A pass that touches them,
    /// removes a whole block, redirects an edge, or rewrites a terminator is
    /// `Cfg`, not `OpsOnly`. The default-pipeline `OpsOnly` passes are verified
    /// to honor this (they only remove IncRef/DecRef/arithmetic/copy ops).
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
    fn run(&self, func: &mut TirFunction, am: &mut AnalysisManager, tti: &TargetInfo) -> PassStats;
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
    fn run(&self, func: &mut TirFunction, am: &mut AnalysisManager, tti: &TargetInfo) -> PassStats {
        (self.run)(func, am, tti)
    }
}

/// Construct an [`FnPass`] boxed as `dyn TirPass`.
fn pass(
    name: &'static str,
    mutates: Mutates,
    run: fn(&mut TirFunction, &mut AnalysisManager, &TargetInfo) -> PassStats,
) -> Box<dyn TirPass> {
    Box::new(FnPass { name, mutates, run })
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
        let dump_tir = std::env::var("MOLT_DUMP_IR").is_ok() || std::env::var("TIR_DUMP").is_ok();
        if has_loop_role && dump_tir {
            dump_tir_artifact(func, "pre", &[]);
        }

        let emit_pass_delta = super::pass_delta::emit_enabled();
        let mut pass_delta_before =
            emit_pass_delta.then(|| super::pass_delta::FactProfile::capture(func));

        let mut am = AnalysisManager::new();

        for p in &self.passes {
            let before_delta = pass_delta_before.take();
            let trace_this = trace_func_enabled(&func.name);
            if trace_this {
                eprintln!(
                    "[TIR-PASS] {} before {}: blocks={} ops={}",
                    func.name,
                    p.name(),
                    func.blocks.len(),
                    function_op_count(func),
                );
            }
            let mut stat = p.run(func, &mut am, &self.target_info);
            stat.name = p.name();
            if trace_this {
                eprintln!(
                    "[TIR-PASS] {} after {}: blocks={} ops={} changed={} removed={} added={} facts={}",
                    func.name,
                    p.name(),
                    func.blocks.len(),
                    function_op_count(func),
                    stat.values_changed,
                    stat.ops_removed,
                    stat.ops_added,
                    stat.facts_changed,
                );
            }

            // Invalidate analyses according to the pass's mutation class.
            // FAIL-CLOSED: `Cfg` drops every CFG- and ops-sensitive analysis;
            // `OpsOnly` drops ops-sensitive analyses (def map) while keeping the
            // CFG-structure analyses (op rewrites don't change edges, and the
            // OpsOnly passes provably never add/remove exception-edge-bearing
            // ops — see `build_default_pipeline`); `ReadOnly` keeps everything.
            let mutation_class = p.mutation_class();
            match mutation_class {
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

            if let Some(before) = before_delta {
                let after = super::pass_delta::FactProfile::capture(func);
                super::pass_delta::emit_pass_delta(
                    func,
                    p.name(),
                    mutation_class,
                    &self.target_info,
                    &stat,
                    &before,
                    &after,
                );
                pass_delta_before = Some(after);
            }

            stats.push(stat);
        }

        let total_changes: usize = stats.iter().map(PassStats::total_changes).sum();
        if total_changes == 0 {
            *func = snapshot.clone();
        }

        if has_loop_role && dump_tir {
            dump_tir_artifact(func, "post", &stats);
        }

        let trace_verify = trace_func_enabled(&func.name);
        if trace_verify {
            eprintln!(
                "[TIR-VERIFY] {} before verify_function: blocks={} ops={}",
                func.name,
                func.blocks.len(),
                function_op_count(func),
            );
        }
        if let Err(errors) = super::verify::verify_function(func) {
            if dump_tir {
                dump_tir_artifact(func, "verify_error", &stats);
            }
            panic!(
                "[TIR] verification failed after optimization of '{}': {:?}",
                func.name, errors
            );
        }
        if trace_verify {
            eprintln!("[TIR-VERIFY] {} after verify_function", func.name);
            eprintln!("[TIR-VERIFY] {} before verify_exception_regions", func.name);
        }
        if let Err(diagnostics) = super::exception_regions::verify_exception_regions(func) {
            if dump_tir {
                dump_tir_artifact(func, "exception_region_error", &stats);
            }
            panic!(
                "[TIR] exception-region verification failed after optimization of '{}': {:?}",
                func.name, diagnostics
            );
        }
        if trace_verify {
            eprintln!("[TIR-VERIFY] {} after verify_exception_regions", func.name);
        }

        if std::env::var("TIR_OPT_STATS").as_deref() == Ok("1") {
            for s in &stats {
                eprintln!(
                    "[TIR] {}: {} values changed, {} ops removed, {} ops added, {} facts changed",
                    s.name, s.values_changed, s.ops_removed, s.ops_added, s.facts_changed
                );
            }
        }

        stats
    }
}

/// Build the default 28-pass optimization pipeline in the canonical order.
///
/// ## RC drop insertion runs in a SEPARATE final phase — NOT here (round-7)
///
/// The two RC drop-insertion passes (`drop_insertion` + `refcount_elim_post`,
/// design 20) are deliberately NOT part of this pipeline. They run ONCE per
/// function in [`build_drop_pipeline`], invoked by
/// [`crate::tir::module_phase::run_module_pipeline`] as its terminal step —
/// AFTER the E1 inliner and module-slot promotion (and the per-caller / per-
/// promoted re-optimizations those run through THIS pipeline). The reason is
/// structural and load-bearing:
///
/// `module_slot_promotion` hoists a module-global accumulator out of the module
/// dict into a register-carried loop phi (the `total = inc(total)` benchmark
/// shape). Its profitability/soundness gate REFUSES to promote a slot whose loop
/// body contains a refcount barrier op (`DecRef`/`IncRef`) — a finalizer running
/// during the decrement could observe the half-updated slot, so promoting across
/// it is unsound. If drop insertion ran inside this pipeline (at per-function
/// step-1, or inside the inliner's per-caller re-opt), it would seed those
/// `DecRef`/`IncRef` ops into the loop BEFORE promotion runs, and promotion would
/// refuse every module-global accumulator — leaving a per-iteration
/// `module_get_attr` / `module_set_attr` / `dec_ref` round-trip that is ~5×
/// slower than the promoted register flow. Running drops as the FINAL phase, once
/// all module transforms have settled the IR, lets promotion see the clean loop
/// and lets drops land on the final (promoted) shape. This is design 20 §2.1's
/// intent ("runs LAST … so SSA, repr facts, and liveness are all final") made
/// whole-program-correct: LAST means after the module phase, not merely last
/// within one per-function pipeline invocation.
///
/// Phase ordering rationale (unchanged from the legacy pipeline):
/// * **Lowering** devirtualizes iterators/ranges and unrolls fixed-trip loops,
///   exposing concrete control flow to later phases.
/// * **Canonicalization** runs twice (instcombine pattern): once pre-type, once
///   post-unboxing.
/// * **Redundancy** (GVN, LICM) runs after canonicalization and type settling.
/// * **Memory** (escape, refcount, reuse, dead-store) runs after redundancy.
/// * **Value** specialization runs late so it sees the final type lattice.
/// * **Cleanup** (check-exception elim, copy-prop, DCE) runs last. Block-arg
///   pruning deliberately runs only in the terminal drop pipeline, after RC
///   ownership transfer facts have been settled; pruning phi payloads before
///   drop insertion would delete ownership-only edge boundaries before the
///   one phase that can place their release.
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
        pass("iter_devirt", Cfg, |f, _am, _tti| {
            passes::iter_devirt::run(f)
        }),
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
        pass("mem_gvn", OpsOnly, |f, am, _tti| {
            passes::mem_gvn::run(f, am)
        }),
        // SROA promotes the fields of a proven-non-escaping object out of memory
        // and deletes the allocation. Placed AFTER mem_gvn (which forwards every
        // observable typed-slot load to a Copy, so a fully-promotable object's
        // residue is store-only) and BEFORE the later cleanup: SROA removes the
        // stores, and the now-unreferenced ObjectNewBoundStack (not
        // side-effecting) is deleted by the trailing dce pass. OpsOnly: it only
        // removes StoreAttr ops within blocks (no CFG change).
        pass("sroa", OpsOnly, |f, am, _tti| passes::sroa::run(f, am)),
        // ── Value optimization ──────────────────────────────────────
        pass("type_guard_hoist", Cfg, |f, am, _tti| {
            passes::type_guard_hoist::run(f, am)
        }),
        pass("sccp", Cfg, |f, _am, _tti| passes::sccp::run(f)),
        pass("strength_reduction", OpsOnly, |f, _am, _tti| {
            passes::strength_reduction::run(f)
        }),
        pass("fast_math", OpsOnly, |f, _am, _tti| {
            passes::fast_math::run(f)
        }),
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
        pass("copy_prop", OpsOnly, |f, _am, _tti| {
            passes::copy_prop::run(f)
        }),
        pass("dce", Cfg, |f, _am, _tti| passes::dce::run(f)),
        // NOTE: RC drop insertion (`drop_insertion` + `refcount_elim_post`,
        // design 20) is NOT here. It runs in a SEPARATE terminal phase
        // ([`build_drop_pipeline`], invoked by `run_module_pipeline` after the
        // module transforms) so it cannot defeat `module_slot_promotion`. See
        // the `build_default_pipeline` doc comment for the full rationale.
    ];
    PassManager::new(passes, target_info)
}

/// Build the RC drop-insertion pipeline (design 20): the two passes that close
/// the whole-program expression-temporary leak, run as a SEPARATE terminal phase
/// AFTER all per-function optimization AND all module-level transforms (the E1
/// inliner + module-slot promotion + the per-caller / per-promoted re-opts those
/// run through [`build_default_pipeline`]). See `build_default_pipeline`'s doc
/// comment for why these are not part of the default pipeline.
///
/// * `drop_insertion` emits `DecRef` at each owned value's last use and `IncRef`
///   before suspension points + on borrowed phi edges (design §5). It is
///   idempotent (bails on the full-function `drop_inserted` marker, and uses a
///   separate `exception_region_drops_inserted` marker for handler-safe pre-bail
///   drops), so a function re-lifted / re-run through this phase does not
///   double-insert drops.
/// * `refcount_elim_post` then elides the balance-preserving subset of the ops it
///   placed (the deferred-RC / DecRef→Free steps are skipped post-drop — they
///   would delete the lone ownership-release DecRefs that close the leak).
/// * `block_arg_prune_post_drop` removes dead block-argument lanes and their
///   incoming edge payloads after drop insertion has settled the final CFG.
///
/// BACKEND-CONDITIONED ACTIVATION. The drop pass is sound only for backends that
/// consume its `DecRef`/`IncRef` by SSA-value identity and run no competing
/// automatic temp-RC mechanism (see [`target_uses_tir_drop_insertion`]):
///
///   * LLVM  — `llvm_backend/lowering.rs` resolves operands by ValueId
///     (`resolve(id)` → correctly-boxed bits) and has no tracked-var auto-RC.
///   * WASM  — each SimpleIR name maps 1:1 to a uniformly NaN-boxed wasm local;
///     no tracked-var auto-RC. The LIR fast lane lowers `IncRef`/`DecRef`
///     directly (lower_to_wasm.rs).
///   * Luau  — GC-managed; `DecRef`/`IncRef` lower to nothing (no-op).
///   * Native/Cranelift — `function_compiler.rs` carries a value-tracking
///     automatic temp-RC substrate, suppressed only for full-function
///     drop-inserted functions by the design-20 §4.1 `drop_inserted`-marker gate
///     (the ~18 `!drop_inserted` sites in `function_compiler.rs`). The
///     exception-only pre-bail marker deliberately does not feed those gates.
///     Activation is the `target_uses_tir_drop_insertion` flip.
///
/// Reusing a [`PassManager`] here (rather than calling the passes directly) keeps
/// drop insertion under the SAME analysis-invalidation soundness contract,
/// post-phase `verify_function`, and `MOLT_VERIFY_ANALYSIS=1` self-check as the
/// optimization pipeline — `drop_insertion` is `Mutates::Cfg` (it may split a
/// critical edge for the mixed-ownership-phi retain, design §5), so correct
/// invalidation matters.
pub fn build_drop_pipeline(target_info: TargetInfo) -> PassManager {
    use Mutates::*;
    let passes: Vec<Box<dyn TirPass>> = vec![
        // `Cfg`: the mixed-ownership-phi retain (design §ownership / §5) may SPLIT
        // a critical edge (a fresh block carrying the edge-exact `IncRef`) when a
        // predecessor reaches an owned phi via multiple arcs with different args.
        // In the common single-arc case (preheader / if-arm) no edge is split and
        // the pass only inserts ops, but it is the POSSIBILITY of a split that
        // fixes the mutation class. (The straight-line / edge-dying / suspension
        // insertions remain pure op additions that carry no exception edge.)
        pass("drop_insertion", Cfg, |f, am, tti| {
            if target_uses_tir_drop_insertion(tti.target) {
                passes::drop_insertion::run(f, am)
            } else {
                PassStats {
                    name: "drop_insertion",
                    ..Default::default()
                }
            }
        }),
        pass("refcount_elim_post", OpsOnly, |f, am, tti| {
            if target_uses_tir_drop_insertion(tti.target) {
                passes::refcount_elim::run_post_drop(f, am)
            } else {
                PassStats {
                    name: "refcount_elim",
                    ..Default::default()
                }
            }
        }),
        pass("block_arg_prune_post_drop", Cfg, |f, _am, _tti| {
            let mut stats = passes::block_arg_prune::run(f);
            stats.name = "block_arg_prune_post_drop";
            stats
        }),
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
            AnalysisId::Liveness => check!(super::passes::liveness::TirLiveness),
            // CallFacts is CFG/ops-sensitive (a deleted block/op can remove a
            // call site), so the self-check recomputes + compares it like any
            // other cached analysis. In Phase 1a nothing on the per-function
            // pipeline calls `am.get::<CallFactsAnalysis>`, so this arm is only
            // reachable once a consumer caches the (intraprocedural-floor) table;
            // the recompute is that same floor, so cached == fresh holds.
            AnalysisId::CallFacts => check!(super::call_facts::CallFactsAnalysis),
            AnalysisId::ExceptionRegions => check!(super::exception_regions::ExceptionRegions),
        }
    }
}

// ---------------------------------------------------------------------------
// TIR dump artifact (moved verbatim from the legacy run_pipeline)
// ---------------------------------------------------------------------------

fn dump_tir_artifact(func: &TirFunction, phase: &str, stats: &[PassStats]) {
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

    let label = if phase == "pre" {
        "PRE-OPT"
    } else {
        "POST-OPT"
    };
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
                .map(|s| {
                    (
                        s.name,
                        s.values_changed,
                        s.ops_removed,
                        s.ops_added,
                        s.facts_changed,
                    )
                })
                .collect::<Vec<_>>()
        ));
    }

    dump.push('\n');
    dump.push_str(&super::printer::print_function(func));
    dump.push('\n');

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

    /// The default pipeline must preserve the EXACT canonical pass order (28
    /// `run` invocations — canonicalize runs twice). The RC drop-insertion passes
    /// (design 20) are NOT in this pipeline — they run in the separate terminal
    /// [`build_drop_pipeline`] (round-7), and block-argument pruning runs only
    /// there after ownership transfer facts have been settled. Any
    /// reorder/insert/drop is a behavior change and must update this list
    /// deliberately.
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
                "sroa",
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

    /// The RC drop-insertion pipeline (round-7) is the two design-20 passes plus
    /// the post-drop block-argument cleanup, in
    /// order. It is a SEPARATE terminal phase run after the module transforms;
    /// the default pipeline above must NOT contain either pass.
    #[test]
    fn drop_pipeline_is_rc_then_block_arg_cleanup() {
        let pm = build_drop_pipeline(TargetInfo::native_release_fast());
        assert_eq!(
            pm.pass_names(),
            vec![
                "drop_insertion",
                "refcount_elim_post",
                "block_arg_prune_post_drop"
            ]
        );

        // And the default optimization pipeline must NOT carry them (the round-7
        // structural separation — drops must not run mid-transform).
        let opt = build_default_pipeline(TargetInfo::native_release_fast());
        assert!(
            !opt.pass_names().contains(&"drop_insertion"),
            "drop_insertion must not be in the default optimization pipeline"
        );
        assert!(
            !opt.pass_names().contains(&"refcount_elim_post"),
            "refcount_elim_post must not be in the default optimization pipeline"
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

        // drop_insertion (in the separate drop pipeline) may split a critical
        // edge for the mixed-ownership-phi retain (design 20 §ownership / §5), so
        // it is declared `Cfg`; block_arg_prune_post_drop rewrites block
        // signatures and edge payloads, so it is also `Cfg`.
        let dp = build_drop_pipeline(TargetInfo::native_release_fast());
        let dp_cfg: Vec<&'static str> = dp
            .passes
            .iter()
            .filter(|p| p.mutation_class() == Mutates::Cfg)
            .map(|p| p.name())
            .collect();
        assert_eq!(dp_cfg, vec!["drop_insertion", "block_arg_prune_post_drop"]);
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
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
        func.blocks.insert(
            header,
            TirBlock {
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
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let pm = build_default_pipeline(TargetInfo::native_release_fast());
        // Force the per-pass analysis self-check on for this run.
        let stats = pm.run_inner(&mut func, true);
        // All 28 optimization-pipeline pass invocations ran (canonicalize runs
        // twice). The RC drop-insertion passes are NOT in this pipeline (round-7
        // moved them to the separate terminal `build_drop_pipeline`), and
        // block-argument pruning waits for that terminal phase.
        assert_eq!(stats.len(), 28);

        // The drop pipeline runs its two passes under the same verify guard.
        // (This trivial loop carries no heap-allocated values, so drop_insertion
        // inserts nothing; both passes still RUN and report stats.)
        let dp = build_drop_pipeline(TargetInfo::native_release_fast());
        let dstats = dp.run_inner(&mut func, true);
        assert_eq!(dstats.len(), 3);
    }

    fn exception_match_ref_without_reachable_pop_function() -> TirFunction {
        let mut func = TirFunction::new("bad_exception_region".into(), vec![], TirType::None);
        let handler = func.fresh_block();
        let exc = func.fresh_value();
        func.label_id_map.insert(handler.0, 9);

        let mut try_start = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryStart,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        };
        try_start
            .attrs
            .insert("value".into(), crate::tir::ops::AttrValue::Int(9));
        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };

        let mut match_ref = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![exc],
            attrs: AttrDict::new(),
            source_span: None,
        };
        match_ref.attrs.insert(
            "_original_kind".into(),
            crate::tir::ops::AttrValue::Str("exception_last_pending".into()),
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![match_ref],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func
    }

    fn ambiguous_exception_match_ref_depth_function() -> TirFunction {
        let mut func = TirFunction::new("ambiguous_exception_region".into(), vec![], TirType::None);
        let before_try = func.fresh_block();
        let handler = func.fresh_block();
        let cond = func.fresh_value();
        let exc = func.fresh_value();
        func.label_id_map.insert(handler.0, 7);

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![cond],
            attrs: AttrDict::new(),
            source_span: None,
        }];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: before_try,
            then_args: vec![],
            else_block: handler,
            else_args: vec![],
        };

        let mut try_start = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryStart,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        };
        try_start
            .attrs
            .insert("value".into(), crate::tir::ops::AttrValue::Int(7));
        func.blocks.insert(
            before_try,
            TirBlock {
                id: before_try,
                args: vec![],
                ops: vec![try_start],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut match_ref = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![exc],
            attrs: AttrDict::new(),
            source_span: None,
        };
        match_ref.attrs.insert(
            "_original_kind".into(),
            crate::tir::ops::AttrValue::Str("exception_last_pending".into()),
        );
        let mut exception_pop = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        };
        exception_pop.attrs.insert(
            "_original_kind".into(),
            crate::tir::ops::AttrValue::Str("exception_pop".into()),
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![match_ref, exception_pop],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func
    }

    #[test]
    #[should_panic(expected = "exception-region verification failed")]
    fn pass_manager_rejects_exception_match_ref_without_reachable_pop() {
        let mut func = exception_match_ref_without_reachable_pop_function();
        let pm = PassManager::new(Vec::new(), TargetInfo::native_release_fast());
        let _ = pm.run_inner(&mut func, true);
    }

    #[test]
    #[should_panic(expected = "exception-region verification failed")]
    fn pass_manager_rejects_ambiguous_exception_match_ref_depth() {
        let mut func = ambiguous_exception_match_ref_depth_function();
        let pm = PassManager::new(Vec::new(), TargetInfo::native_release_fast());
        let _ = pm.run_inner(&mut func, true);
    }

    #[test]
    #[should_panic(expected = "exception-region verification failed")]
    fn drop_pipeline_rejects_exception_region_diagnostics_before_codegen() {
        let mut func = exception_match_ref_without_reachable_pop_function();
        let pm = build_drop_pipeline(TargetInfo::native_release_fast());
        let _ = pm.run_inner(&mut func, true);
    }
}
