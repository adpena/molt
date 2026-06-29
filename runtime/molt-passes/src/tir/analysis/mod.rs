//! Per-function analysis manager for the TIR pass pipeline.
//!
//! This is Tier-0 substrate **S1** of the compiler-foundation program (an
//! LLVM new-PM `AnalysisManager` analog). Before it existed, the dominator
//! tree, predecessor map, reachability sets and per-header loop bodies were
//! recomputed independently in GVN, LICM, BCE, refcount-elim, type-guard-hoist
//! and block-versioning — roughly seven O(n²) dominator passes per pipeline
//! run, plus two more duplicate dominator implementations in `verify.rs`.
//!
//! The [`AnalysisManager`] computes each analysis **lazily** the first time a
//! pass asks for it and caches the result for the remainder of the function's
//! pipeline. When a pass mutates the CFG, the [`PassManager`](crate::tir::pass_manager)
//! calls [`AnalysisManager::invalidate_cfg`], dropping every CFG-sensitive
//! analysis so the next consumer recomputes against the new shape.
//!
//! ## Soundness model: FAIL-CLOSED
//!
//! Every cached analysis declares [`Analysis::CFG_SENSITIVE`]. The invalidation
//! contract is:
//!
//! * A CFG-mutating pass MUST be classified `Mutates::Cfg`, after which the
//!   PassManager drops every CFG-sensitive cache entry. A *missing* mutation
//!   declaration would leave a stale entry → a miscompile. The default pass
//!   mutation class is therefore `Mutates::Cfg` (over-invalidate, never
//!   under-invalidate): the worst case is a redundant recompute (a cold miss),
//!   **never** a stale-cache miscompile.
//! * There is no partial invalidation that could leave a dependent analysis
//!   stale while its dependency is fresh: `invalidate_cfg` clears *all*
//!   CFG-sensitive analyses atomically. `DomChildren` derives from
//!   `ImmediateDoms` which derives from `PredMap`; all three are CFG-sensitive
//!   and cleared together.
//!
//! A debug self-check (`MOLT_VERIFY_ANALYSIS=1`, see
//! [`crate::tir::pass_manager`]) recomputes each analysis from scratch after a
//! CFG-mutating pass and asserts it matches what a fresh manager would produce,
//! catching any pass that mutates the CFG without declaring it.
//!
//! ## Analyses and their consumers
//!
//! | Analysis        | Underlying `dominators.rs` fn        | Consumers                        |
//! |-----------------|--------------------------------------|----------------------------------|
//! | [`PredMap`]      | `build_pred_map` (full CFG)         | gvn, licm, bce, refcount, …      |
//! | [`ImmediateDoms`]| `compute_idoms` (full CFG)          | gvn, licm, bce, refcount         |
//! | [`DomChildren`]  | `build_dom_children`                | gvn                              |
//! | [`ExecReachable`]| `executable_reachable_blocks`       | (full-CFG reachable set)         |
//! | [`StrictReachable`]| `reachable_blocks_with(TerminatorOnly)` | gvn (cross-block replace) |
//! | [`LoopForest`]   | loop roles/backedges + `collect_loop_blocks` | licm, bce, vectorize, polyhedral |
//! | [`DefMap`]       | value → defining block              | gvn, licm                        |
//!
//! The names map to the S1 spec's `{PredMap, ImmediateDoms, DomChildren,
//! ExecReachable, MetaReachable, LoopForest, DefMap}` — `MetaReachable` is the
//! strict terminator-only reachability view (`StrictReachable` here, the
//! verifier/`verify_lir` view), distinguished from the full-CFG `ExecReachable`
//! that also follows implicit exception edges.

use std::any::Any;
use std::collections::{HashMap, HashSet};

use super::blocks::BlockId;
use super::dominators::{self, CfgEdgePolicy};
use super::function::TirFunction;
use super::values::ValueId;

/// Stable identifier for each analysis the manager can cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnalysisId {
    PredMap,
    ImmediateDoms,
    DomChildren,
    ExecReachable,
    StrictReachable,
    LoopForest,
    DefMap,
    /// Scalar-evolution recurrences + trip counts (Tier-0 S6).
    ScalarEvolution,
    /// Integer value-range / interval lattice (Tier-0 S6).
    ValueRange,
    /// First-class alias analysis: points-to/escape map + transparent-copy
    /// alias roots + memory-region/load-purity queries (Tier-0 S5 phase 1).
    AliasAnalysis,
    /// MemorySSA: memory versions (Def/Use/Phi nodes) over the alias oracle's
    /// regions — which store produced the value a load reads (Tier-0 S5
    /// phase 2a).
    MemorySSA,
    /// Backward-dataflow liveness with representation-filtered live sets — the
    /// last-use map the RC drop-insertion pass consumes (design 20, Phase 2).
    Liveness,
    /// Per-call-site fact records (target / typed-return / leaf / no-throw /
    /// inlinable), keyed by each call op's result `ValueId` — the CallFacts
    /// side-table (foundation design 47). The precise table is built
    /// interprocedurally in the module phase and prepopulated; the cached
    /// [`Analysis::compute`] path is the fail-closed intraprocedural floor.
    CallFacts,
    /// Backend-neutral exception handler ownership facts: match-ref producers,
    /// reachable path-depth `exception_pop` release boundaries, and diagnostics
    /// for missing/ambiguous/too-early release (foundation design 45).
    ExceptionRegions,
}

impl AnalysisId {
    /// All analyses, for iteration in the debug self-check.
    pub const ALL: [AnalysisId; 14] = [
        AnalysisId::PredMap,
        AnalysisId::ImmediateDoms,
        AnalysisId::DomChildren,
        AnalysisId::ExecReachable,
        AnalysisId::StrictReachable,
        AnalysisId::LoopForest,
        AnalysisId::DefMap,
        AnalysisId::ScalarEvolution,
        AnalysisId::ValueRange,
        AnalysisId::AliasAnalysis,
        AnalysisId::MemorySSA,
        AnalysisId::Liveness,
        AnalysisId::CallFacts,
        AnalysisId::ExceptionRegions,
    ];
}

/// An analysis is a pure function of a [`TirFunction`] whose result the manager
/// memoizes. Implementors are zero-sized marker types; the manager keys cache
/// slots on [`Analysis::ID`].
pub trait Analysis {
    /// The computed value cached by the manager.
    type Result: Any + Send + Sync;

    /// Stable cache key.
    const ID: AnalysisId;

    /// Whether this analysis becomes invalid when the CFG (block set, edges, or
    /// terminators / `loop_roles`) changes. The dominator/pred/reachability/
    /// loop-forest analyses are CFG-sensitive; a [`DefMap`] additionally
    /// depends on ops (see [`Analysis::OPS_SENSITIVE`]). No default — every
    /// analysis declares it explicitly (fail-closed: an undeclared analysis
    /// won't compile).
    const CFG_SENSITIVE: bool;

    /// Whether this analysis becomes invalid when *ops within blocks* change
    /// (an op added/removed/rewritten such that a value's defining block or set
    /// of defined values changes). Only [`DefMap`] is ops-sensitive; the
    /// dominator/pred/reachability analyses depend only on CFG edges and are
    /// unaffected by op rewrites. FAIL-CLOSED via the mutation-class default:
    /// any pass that may change ops is at least `OpsOnly`, which drops every
    /// ops-sensitive analysis.
    const OPS_SENSITIVE: bool;

    /// Compute the analysis from scratch.
    fn compute(func: &TirFunction) -> Self::Result;
}

/// Loop forest: structural loop headers from explicit `loop_roles` plus
/// dominator-proven backedges, with the natural-loop body of each. Shared by
/// LICM, BCE, vectorize, and polyhedral so loop-shape discovery has one cached
/// authority.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopForestResult {
    /// Loop headers in ascending `BlockId` order (deterministic).
    pub headers: Vec<BlockId>,
    /// header → natural-loop body block set.
    pub bodies: HashMap<BlockId, HashSet<BlockId>>,
}

// ---------------------------------------------------------------------------
// Analysis marker types
// ---------------------------------------------------------------------------

/// Predecessor map over the full CFG (terminator + exception edges).
pub struct PredMap;
impl Analysis for PredMap {
    type Result = HashMap<BlockId, Vec<BlockId>>;
    const ID: AnalysisId = AnalysisId::PredMap;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = false;
    fn compute(func: &TirFunction) -> Self::Result {
        dominators::build_pred_map(func)
    }
}

/// Immediate-dominator tree over the full CFG.
pub struct ImmediateDoms;
impl Analysis for ImmediateDoms {
    type Result = HashMap<BlockId, Option<BlockId>>;
    const ID: AnalysisId = AnalysisId::ImmediateDoms;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = false;
    fn compute(func: &TirFunction) -> Self::Result {
        let pred_map = dominators::build_pred_map(func);
        dominators::compute_idoms(func, &pred_map)
    }
}

/// Dominator-tree children map (derived from [`ImmediateDoms`]).
pub struct DomChildren;
impl Analysis for DomChildren {
    type Result = HashMap<BlockId, Vec<BlockId>>;
    const ID: AnalysisId = AnalysisId::DomChildren;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = false;
    fn compute(func: &TirFunction) -> Self::Result {
        let pred_map = dominators::build_pred_map(func);
        let idoms = dominators::compute_idoms(func, &pred_map);
        dominators::build_dom_children(&idoms)
    }
}

/// Full-CFG reachable block set (follows implicit exception edges).
pub struct ExecReachable;
impl Analysis for ExecReachable {
    type Result = HashSet<BlockId>;
    const ID: AnalysisId = AnalysisId::ExecReachable;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = false;
    fn compute(func: &TirFunction) -> Self::Result {
        dominators::executable_reachable_blocks(func)
    }
}

/// Strict terminator-only reachable block set (the verifier / `verify_lir`
/// view). Blocks reachable *only* via exception edges are excluded.
pub struct StrictReachable;
impl Analysis for StrictReachable {
    type Result = HashSet<BlockId>;
    const ID: AnalysisId = AnalysisId::StrictReachable;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = false;
    fn compute(func: &TirFunction) -> Self::Result {
        dominators::reachable_blocks_with(func, CfgEdgePolicy::TerminatorOnly)
    }
}

/// Loop forest (headers from explicit loop roles and natural-loop backedges,
/// bodies from natural-loop construction).
pub struct LoopForest;
impl Analysis for LoopForest {
    type Result = LoopForestResult;
    const ID: AnalysisId = AnalysisId::LoopForest;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = false;
    fn compute(func: &TirFunction) -> Self::Result {
        let pred_map = dominators::build_pred_map(func);
        let idoms = dominators::compute_idoms(func, &pred_map);

        let mut header_set: HashSet<BlockId> = func
            .loop_roles
            .iter()
            .filter_map(|(bid, role)| {
                if matches!(role, super::blocks::LoopRole::LoopHeader)
                    && func.blocks.contains_key(bid)
                {
                    Some(*bid)
                } else {
                    None
                }
            })
            .collect();

        for (&candidate, preds) in &pred_map {
            if idoms.contains_key(&candidate)
                && preds
                    .iter()
                    .any(|&pred| dominators::dominates(candidate, pred, &idoms))
            {
                header_set.insert(candidate);
            }
        }

        let mut headers: Vec<BlockId> = header_set.into_iter().collect();
        headers.sort_unstable_by_key(|b| b.0);

        let mut bodies: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
        for &h in &headers {
            bodies.insert(
                h,
                dominators::collect_loop_blocks(func, &pred_map, &idoms, h),
            );
        }
        LoopForestResult { headers, bodies }
    }
}

/// Map from `ValueId` to the `BlockId` that defines it (block arg or op
/// result). Function parameters are defined at the entry block.
pub struct DefMap;
impl Analysis for DefMap {
    type Result = HashMap<ValueId, BlockId>;
    const ID: AnalysisId = AnalysisId::DefMap;
    // DefMap depends on op results / block args, so it is both CFG-sensitive
    // (removing a block removes its defs) and ops-sensitive (removing an op
    // removes its defs).
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        let mut def_map: HashMap<ValueId, BlockId> = HashMap::new();
        for (&bid, block) in &func.blocks {
            for arg in &block.args {
                def_map.insert(arg.id, bid);
            }
            for op in &block.ops {
                for &res in &op.results {
                    def_map.insert(res, bid);
                }
            }
        }
        // Function parameters are defined "before" the entry block. Use
        // `entry` insertion only if not already produced inside the body
        // (a result can never alias a param id in valid SSA, but stay defensive).
        for i in 0..func.param_types.len() {
            def_map.entry(ValueId(i as u32)).or_insert(func.entry_block);
        }
        def_map
    }
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Per-function analysis cache with CFG-aware invalidation.
///
/// One `AnalysisManager` is created per function by the
/// [`PassManager`](crate::tir::pass_manager) and threaded through every pass.
/// It is **not** shared across functions (analyses are per-function) and is not
/// `Send`/`Sync`-shared — the parallel module compiles distinct functions on
/// distinct threads, each with its own manager.
#[derive(Default)]
pub struct AnalysisManager {
    /// Cached results, type-erased behind `Any`. The concrete type stored under
    /// `AnalysisId::X` is always `X::Result`, guaranteed by `get::<X>`.
    cache: HashMap<AnalysisId, Box<dyn Any + Send + Sync>>,
}

impl AnalysisManager {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Get the cached result of analysis `A`, computing and caching it on a
    /// miss. The borrow is tied to `&self`; callers that need a `&mut` pass
    /// must drop the analysis borrow first (the pipeline does: it reads
    /// analyses, then mutates the function).
    pub fn get<A: Analysis>(&mut self, func: &TirFunction) -> &A::Result {
        let id = A::ID;
        self.cache
            .entry(id)
            .or_insert_with(|| Box::new(A::compute(func)) as Box<dyn Any + Send + Sync>);
        // SAFETY of the downcast: the only writer of slot `id` is this method,
        // which always stores `A::Result`. The cache key `A::ID` is unique per
        // `A`. Therefore the stored `Any` is always `A::Result`.
        self.cache
            .get(&id)
            .and_then(|boxed| boxed.downcast_ref::<A::Result>())
            .expect("analysis cache type invariant violated")
    }

    /// Prepopulate the cache with an already-computed result for analysis `A`.
    /// Used to seed the manager from a computation the caller already performed
    /// (e.g. type-refinement's dominator tree) so the first pipeline pass that
    /// needs it gets a cache hit. The caller asserts the value is valid for the
    /// function's *current* CFG shape.
    pub fn prepopulate<A: Analysis>(&mut self, result: A::Result) {
        self.cache.insert(A::ID, Box::new(result));
    }

    /// Drop every CFG-sensitive analysis (which, by the dependency model,
    /// implies dropping everything ops-sensitive too — a block removal removes
    /// its ops' defs). Called by the PassManager after any `Mutates::Cfg` pass.
    /// FAIL-CLOSED: clears all affected entries atomically so no derived
    /// analysis can outlive its dependency.
    pub fn invalidate_cfg(&mut self) {
        self.cache
            .retain(|&id, _| !cfg_sensitive(id) && !ops_sensitive(id));
    }

    /// Drop every ops-sensitive analysis (but keep CFG-structure analyses,
    /// which op rewrites do not affect). Called by the PassManager after any
    /// `Mutates::OpsOnly` pass.
    pub fn invalidate_ops(&mut self) {
        self.cache.retain(|&id, _| !ops_sensitive(id));
    }

    /// Drop a single analysis (and nothing else). Use sparingly — prefer
    /// [`invalidate_cfg`](Self::invalidate_cfg) which is fail-closed across the
    /// whole affected set.
    pub fn invalidate(&mut self, id: AnalysisId) {
        self.cache.remove(&id);
    }

    /// True if analysis `id` is currently cached. Test/debug aid.
    pub fn is_cached(&self, id: AnalysisId) -> bool {
        self.cache.contains_key(&id)
    }
}

/// CFG-sensitivity by id — the table the manager consults on invalidation.
/// Mirrors each analysis's `CFG_SENSITIVE` const. Kept exhaustive so adding an
/// `AnalysisId` variant without classifying it fails to compile.
fn cfg_sensitive(id: AnalysisId) -> bool {
    use super::call_facts::CallFactsAnalysis;
    use super::exception_regions::ExceptionRegions;
    use super::passes::alias_analysis::AliasAnalysis;
    use super::passes::liveness::TirLiveness;
    use super::passes::memory_ssa::MemorySSA;
    use super::passes::scev::ScalarEvolution;
    use super::passes::value_range::ValueRange;
    match id {
        AnalysisId::PredMap => PredMap::CFG_SENSITIVE,
        AnalysisId::ImmediateDoms => ImmediateDoms::CFG_SENSITIVE,
        AnalysisId::DomChildren => DomChildren::CFG_SENSITIVE,
        AnalysisId::ExecReachable => ExecReachable::CFG_SENSITIVE,
        AnalysisId::StrictReachable => StrictReachable::CFG_SENSITIVE,
        AnalysisId::LoopForest => LoopForest::CFG_SENSITIVE,
        AnalysisId::DefMap => DefMap::CFG_SENSITIVE,
        AnalysisId::ScalarEvolution => ScalarEvolution::CFG_SENSITIVE,
        AnalysisId::ValueRange => ValueRange::CFG_SENSITIVE,
        AnalysisId::AliasAnalysis => AliasAnalysis::CFG_SENSITIVE,
        AnalysisId::MemorySSA => MemorySSA::CFG_SENSITIVE,
        AnalysisId::Liveness => TirLiveness::CFG_SENSITIVE,
        AnalysisId::CallFacts => CallFactsAnalysis::CFG_SENSITIVE,
        AnalysisId::ExceptionRegions => ExceptionRegions::CFG_SENSITIVE,
    }
}

/// Ops-sensitivity by id — mirrors each analysis's `OPS_SENSITIVE` const.
fn ops_sensitive(id: AnalysisId) -> bool {
    use super::call_facts::CallFactsAnalysis;
    use super::exception_regions::ExceptionRegions;
    use super::passes::alias_analysis::AliasAnalysis;
    use super::passes::liveness::TirLiveness;
    use super::passes::memory_ssa::MemorySSA;
    use super::passes::scev::ScalarEvolution;
    use super::passes::value_range::ValueRange;
    match id {
        AnalysisId::PredMap => PredMap::OPS_SENSITIVE,
        AnalysisId::ImmediateDoms => ImmediateDoms::OPS_SENSITIVE,
        AnalysisId::DomChildren => DomChildren::OPS_SENSITIVE,
        AnalysisId::ExecReachable => ExecReachable::OPS_SENSITIVE,
        AnalysisId::StrictReachable => StrictReachable::OPS_SENSITIVE,
        AnalysisId::LoopForest => LoopForest::OPS_SENSITIVE,
        AnalysisId::DefMap => DefMap::OPS_SENSITIVE,
        AnalysisId::ScalarEvolution => ScalarEvolution::OPS_SENSITIVE,
        AnalysisId::ValueRange => ValueRange::OPS_SENSITIVE,
        AnalysisId::AliasAnalysis => AliasAnalysis::OPS_SENSITIVE,
        AnalysisId::MemorySSA => MemorySSA::OPS_SENSITIVE,
        AnalysisId::Liveness => TirLiveness::OPS_SENSITIVE,
        AnalysisId::CallFacts => CallFactsAnalysis::OPS_SENSITIVE,
        AnalysisId::ExceptionRegions => ExceptionRegions::OPS_SENSITIVE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    /// Linear chain bb0 → bb1 → bb2.
    fn linear() -> TirFunction {
        let mut func = TirFunction::new("lin".into(), vec![], TirType::None);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: bb1,
            args: vec![],
        };
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb2,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func
    }

    /// Diamond bb0 →{bb1,bb2}→ bb3.
    fn diamond() -> TirFunction {
        let mut func = TirFunction::new("dia".into(), vec![], TirType::None);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let bb3 = func.fresh_block();
        let cond = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstBool,
                operands: vec![],
                results: vec![cond],
                attrs: AttrDict::new(),
                source_span: None,
            });
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: bb1,
                then_args: vec![],
                else_block: bb2,
                else_args: vec![],
            };
        }
        for b in [bb1, bb2] {
            func.blocks.insert(
                b,
                TirBlock {
                    id: b,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Branch {
                        target: bb3,
                        args: vec![],
                    },
                },
            );
        }
        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func
    }

    /// Self-loop: bb0 → bb1, bb1 → bb1 (back-edge) / bb1 → bb2.
    fn loopy() -> TirFunction {
        let mut func = TirFunction::new("loop".into(), vec![], TirType::None);
        let header = func.fresh_block();
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
                    then_block: header,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
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
        func
    }

    // -- each analysis equals the underlying dominators.rs fn -----------------

    #[test]
    fn pred_map_matches_underlying_on_all_shapes() {
        for func in [linear(), diamond(), loopy()] {
            let mut am = AnalysisManager::new();
            let cached = am.get::<PredMap>(&func).clone();
            assert_eq!(cached, dominators::build_pred_map(&func));
        }
    }

    #[test]
    fn idoms_match_underlying_on_all_shapes() {
        for func in [linear(), diamond(), loopy()] {
            let mut am = AnalysisManager::new();
            let cached = am.get::<ImmediateDoms>(&func).clone();
            let pred = dominators::build_pred_map(&func);
            assert_eq!(cached, dominators::compute_idoms(&func, &pred));
        }
    }

    #[test]
    fn dom_children_match_underlying() {
        for func in [linear(), diamond(), loopy()] {
            let mut am = AnalysisManager::new();
            let cached = am.get::<DomChildren>(&func).clone();
            let pred = dominators::build_pred_map(&func);
            let idoms = dominators::compute_idoms(&func, &pred);
            assert_eq!(cached, dominators::build_dom_children(&idoms));
        }
    }

    #[test]
    fn exec_vs_strict_reachable() {
        for func in [linear(), diamond(), loopy()] {
            let mut am = AnalysisManager::new();
            assert_eq!(
                *am.get::<ExecReachable>(&func),
                dominators::executable_reachable_blocks(&func)
            );
            assert_eq!(
                *am.get::<StrictReachable>(&func),
                dominators::reachable_blocks_with(&func, CfgEdgePolicy::TerminatorOnly)
            );
        }
    }

    #[test]
    fn loop_forest_headers_and_bodies() {
        let func = loopy();
        let mut am = AnalysisManager::new();
        let forest = am.get::<LoopForest>(&func).clone();
        // Single self-loop header.
        assert_eq!(forest.headers.len(), 1);
        let h = forest.headers[0];
        let body = &forest.bodies[&h];
        assert!(body.contains(&h));
    }

    #[test]
    fn loop_forest_discovers_backedge_headers_without_loop_roles() {
        let mut func = loopy();
        func.loop_roles.clear();

        let mut am = AnalysisManager::new();
        let forest = am.get::<LoopForest>(&func).clone();

        assert_eq!(forest.headers.len(), 1);
        let h = forest.headers[0];
        let body = &forest.bodies[&h];
        assert!(body.contains(&h));
    }

    #[test]
    fn def_map_covers_args_results_params() {
        let mut func = TirFunction::new("d".into(), vec![TirType::I64], TirType::None);
        let v = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![v],
                attrs: AttrDict::new(),
                source_span: None,
            });
            entry.terminator = Terminator::Return { values: vec![] };
        }
        let mut am = AnalysisManager::new();
        let def = am.get::<DefMap>(&func).clone();
        assert_eq!(def[&v], func.entry_block);
        // Param 0 defined at entry.
        assert_eq!(def[&ValueId(0)], func.entry_block);
    }

    // -- caching + invalidation -----------------------------------------------

    #[test]
    fn get_caches_result() {
        let func = diamond();
        let mut am = AnalysisManager::new();
        assert!(!am.is_cached(AnalysisId::PredMap));
        let _ = am.get::<PredMap>(&func);
        assert!(am.is_cached(AnalysisId::PredMap));
    }

    #[test]
    fn invalidate_cfg_drops_cfg_sensitive() {
        let func = diamond();
        let mut am = AnalysisManager::new();
        let _ = am.get::<PredMap>(&func);
        let _ = am.get::<ImmediateDoms>(&func);
        let _ = am.get::<DefMap>(&func);
        assert!(am.is_cached(AnalysisId::PredMap));
        am.invalidate_cfg();
        for id in AnalysisId::ALL {
            assert!(!am.is_cached(id), "{:?} should be dropped", id);
        }
    }

    #[test]
    fn invalidation_returns_fresh_not_stale() {
        // Cache the pred map on a diamond, then mutate the CFG (add an edge),
        // invalidate, and confirm the recomputed pred map reflects the new
        // shape — i.e. the manager returns a freshly-computed result, not the
        // stale cached one.
        let mut func = diamond();
        let mut am = AnalysisManager::new();
        let bb3 = *func.blocks.keys().max_by_key(|b| b.0).unwrap();
        let before = am
            .get::<PredMap>(&func)
            .get(&bb3)
            .cloned()
            .unwrap_or_default();
        let before_len = before.len();

        // Mutate: redirect bb1 to also create a new predecessor structure.
        // Add a fresh block bb4 that branches to bb3, and point bb3's args path
        // by making one of the diamond arms route through bb4.
        let bb4 = func.fresh_block();
        // Find an arm (the then-arm) and redirect it through bb4.
        let entry = func.entry_block;
        let then_target = match &func.blocks[&entry].terminator {
            Terminator::CondBranch { then_block, .. } => *then_block,
            _ => unreachable!(),
        };
        // then_target currently branches to bb3; insert bb4 between by adding a
        // brand-new predecessor edge bb4 → bb3 and routing then_target → bb4.
        func.blocks.get_mut(&then_target).unwrap().terminator = Terminator::Branch {
            target: bb4,
            args: vec![],
        };
        func.blocks.insert(
            bb4,
            TirBlock {
                id: bb4,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );

        am.invalidate_cfg();
        let after = am
            .get::<PredMap>(&func)
            .get(&bb3)
            .cloned()
            .unwrap_or_default();
        // bb3's predecessors changed: previously {bb1,bb2}, now {bb2,bb4}.
        assert!(
            after.contains(&bb4),
            "fresh pred map must include the new edge"
        );
        assert!(!after.contains(&then_target), "stale edge must be gone");
        assert_eq!(
            after.len(),
            before_len,
            "still two predecessors, but different ones"
        );
    }

    #[test]
    fn prepopulate_yields_cache_hit() {
        let func = diamond();
        let mut am = AnalysisManager::new();
        let pred = dominators::build_pred_map(&func);
        am.prepopulate::<PredMap>(pred.clone());
        assert!(am.is_cached(AnalysisId::PredMap));
        assert_eq!(*am.get::<PredMap>(&func), pred);
    }
}
