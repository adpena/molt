//! Whole-program **module pass phase** (Tier-0 substrate **S4**).
//!
//! Before this module existed, the TIR pipeline was *strictly per-function*:
//! [`crate::tir::parallel::compile_module_parallel`] ran the S1
//! [`PassManager`](crate::tir::pass_manager::PassManager) over each function
//! independently, and the only whole-program structure (the call graph, the leaf
//! set) lived in the SimpleIR layer of the native backend. There was no place to
//! compute a module-level analysis that the interprocedural tier (inliner E1,
//! IP-escape E3, IPSCCP E4, monomorphization E5) could read.
//!
//! [`run_module_pipeline`] is that place. It runs **once, before** the
//! per-function pipeline, over the whole [`TirModule`], and produces a
//! [`ModuleAnalysis`] — the call graph plus bottom-up function summaries — that
//! whole-program consumers read. It is the module-scope analog of S1's
//! per-function [`AnalysisManager`](crate::tir::analysis::AnalysisManager): a
//! single, shared, lazily-built source of interprocedural truth.
//!
//! ## How it composes with S1 + S2 + `parallel.rs` (the reconciliation)
//!
//! This phase **does not fork a parallel pipeline.** The contract is:
//!
//! 1. The driver lifts every function to TIR and assembles a [`TirModule`].
//! 2. `run_module_pipeline(&mut module)` runs here, producing [`ModuleAnalysis`]
//!    (read-only over the module — it builds the call graph + summaries, it does
//!    not transform function bodies). The `&mut` is reserved for the future
//!    module *transforms* (inlining splices bodies across functions); today the
//!    phase is analysis-only, so it leaves bodies untouched.
//! 3. The driver then runs the existing per-function pipeline
//!    ([`compile_module_parallel`](crate::tir::parallel::compile_module_parallel),
//!    rayon work-stealing, each function threading its own S1
//!    [`AnalysisManager`] and the shared S2
//!    [`TargetInfo`](crate::tir::target_info::TargetInfo)). The module phase
//!    runs *before* this and its result is available for the duration of module
//!    compilation.
//!
//! The S2 [`TargetInfo`] threads through unchanged: it is the same `&TargetInfo`
//! that `compile_module_parallel` already shares with every function, and it is
//! passed here so a future cost-model-gated module transform (the inliner's
//! per-call ROI) consults the one source of truth rather than a fresh constant.
//!
//! ## The native-backend caveat (batching / per-function roundtrip)
//!
//! The native driver compiles in two shapes (see `native_backend::simple_backend`
//! and `main.rs`):
//!
//! * **Non-batched**: the whole program is one object; the leaf set is computed
//!   over all functions.
//! * **Batched** (>64 functions): `main.rs` builds ONE whole-program module
//!   context (including the leaf set) over *all* functions, then partitions into
//!   batches that each receive that same whole-program context. So even in the
//!   batched shape the leaf set this phase computes is whole-program — sound.
//!
//! Where a batch backend recomputes analysis over only its own function subset
//! (the per-batch fallback when no module context is threaded), the call graph
//! sees only that subset: any call to a function outside the batch resolves to
//! [`crate::tir::call_graph::CallEdge::Opaque`] (the named callee is not in this
//! sub-module), which *disqualifies* leaf-ness — strictly conservative, never
//! unsound. A function the per-batch graph would have called a leaf can only be
//! "more leaf" than the whole-program truth allows, and since opaque calls block
//! leaf-ness, the per-batch set is a subset of the whole-program set. Skipping
//! the recursion guard on a subset of the truly-safe leaves is sound (it just
//! forgoes an optimization on the cross-batch callers).

use super::call_graph::CallGraph;
use super::function::TirModule;
use super::passes::ip_summary::ModuleSummaries;
use super::target_info::TargetInfo;
use std::time::Instant;

#[derive(Debug, Clone)]
struct ModuleStageAuditShape {
    functions: usize,
    tir_blocks: usize,
    tir_ops: usize,
    largest_function: String,
    largest_ops: usize,
}

fn module_stage_audit_enabled() -> bool {
    std::env::var("MOLT_MODULE_STAGE_AUDIT").as_deref() == Ok("1")
        || std::env::var("MOLT_WASM_STAGE_AUDIT").as_deref() == Ok("1")
}

fn module_stage_shape(module: &TirModule) -> ModuleStageAuditShape {
    let mut tir_blocks = 0usize;
    let mut tir_ops = 0usize;
    let mut largest_function = "<none>".to_string();
    let mut largest_ops = 0usize;
    for func in &module.functions {
        let blocks = func.blocks.len();
        let ops = func
            .blocks
            .values()
            .fold(0usize, |total, block| total.saturating_add(block.ops.len()));
        tir_blocks = tir_blocks.saturating_add(blocks);
        tir_ops = tir_ops.saturating_add(ops);
        if ops > largest_ops {
            largest_ops = ops;
            largest_function = func.name.clone();
        }
    }
    ModuleStageAuditShape {
        functions: module.functions.len(),
        tir_blocks,
        tir_ops,
        largest_function,
        largest_ops,
    }
}

fn emit_module_stage_audit(
    stage: &str,
    module: &TirModule,
    changed_functions: Option<usize>,
    elapsed_ms: u128,
) {
    if !module_stage_audit_enabled() {
        return;
    }
    let shape = module_stage_shape(module);
    eprintln!(
        "[molt-module-stage-audit] stage={stage} functions={} tir_blocks={} tir_ops={} largest_function={} largest_ops={} changed_functions={} elapsed_ms={} peak_rss_mib={}",
        shape.functions,
        shape.tir_blocks,
        shape.tir_ops,
        shape.largest_function,
        shape.largest_ops,
        changed_functions
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        elapsed_ms,
        crate::process_diagnostics::process_peak_rss_mib_label(),
    );
}

/// The result of the whole-program module phase: the interprocedural analysis
/// the IPO tier reads. Computed once per module, shared read-only.
#[derive(Debug, Clone, Default)]
pub struct ModuleAnalysis {
    /// The whole-program call graph (static-direct + opaque edges, SCC /
    /// bottom-up order / recursive set / leaf set).
    pub call_graph: CallGraph,
    /// Bottom-up function summaries (leaf / op-count / return-type), one per
    /// function in the module.
    pub summaries: ModuleSummaries,
    /// Names of the functions the inliner CHANGED (had ≥1 callee spliced in).
    /// Production codegen back-converts ONLY these functions' (post-inline) TIR
    /// to SimpleIR; every other function keeps its byte-identical per-function
    /// pipeline output (no redundant second TIR roundtrip).
    pub changed_functions: Vec<String>,
    /// **CallFacts** side-tables, one per function (keyed by function name), each
    /// keyed internally by a call op's result `ValueId` (foundation design 47).
    /// The IR-fact half of doc 46 §4.1 (FactGraph): the typed call target, typed
    /// return, leaf / no-throw / inline-eligibility facts that backends and the
    /// `tools/call_fact_coverage.py` census read. Built over the **post-inline**
    /// module (same rebuild as `call_graph` / `summaries`), so the recorded facts
    /// describe the program the backends actually lower. Phase 1a attaches these
    /// (consumed by nothing on the hot path → byte-identical); later phases route
    /// call lowering / RC elision / devirt through them.
    pub call_facts: std::collections::BTreeMap<String, super::call_facts::CallFactsTable>,
}

impl ModuleAnalysis {
    /// The leaf-function set — functions that make no call of any kind and so
    /// cannot recurse. This is the SOUND, strictly-more-precise replacement for
    /// the native backend's legacy SimpleIR "has no call op" leaf scan.
    pub fn leaf_functions(&self) -> std::collections::BTreeSet<String> {
        self.call_graph.leaf_functions()
    }

    /// The [`CallFactsTable`](super::call_facts::CallFactsTable) for the function
    /// named `name`, if it is in the module. Each entry is keyed by a call op's
    /// result `ValueId` (foundation design 47).
    pub fn call_facts_for(&self, name: &str) -> Option<&super::call_facts::CallFactsTable> {
        self.call_facts.get(name)
    }
}

/// Run the whole-program module pass phase over `module`, returning the
/// [`ModuleAnalysis`] for the duration of module compilation.
///
/// Runs **before** the per-function
/// [`compile_module_parallel`](crate::tir::parallel::compile_module_parallel).
/// The phase:
///
/// 1. Builds the call graph + bottom-up summaries over the lifted module.
/// 2. Runs the **E1 function inliner** ([`run_inliner`](super::passes::inliner::run_inliner)):
///    a module *transform* that splices in-budget, exception-free,
///    non-recursive, non-generator leaf-ish callees at their static call sites,
///    bottom-up, re-optimizing each merged caller via the per-function pipeline.
/// 3. Because step 2 mutates bodies (calls disappear, op counts change), the
///    call graph and summaries are **rebuilt** so the returned [`ModuleAnalysis`]
///    reflects the post-inline module — the leaf set the native backend's
///    recursion-guard skip consults must describe the *inlined* program, not the
///    pre-inline one.
///
/// `tti` is the unified cost model (Tier-0 S2): the single source of truth for
/// the inliner's per-callee budget.
///
/// `non_inlinable` is the set of callee names whose canonical definition is
/// linked externally to this module (e.g. the native/wasm shared-stdlib
/// partition's `stdlib_shared.o` symbols). The inliner treats them as
/// external-linkage functions and never splices their bodies — the caller keeps
/// the external reference. Pass an empty set when every body in the module is
/// locally owned (the common, non-partitioned build).
pub fn run_module_pipeline(
    module: &mut TirModule,
    tti: &TargetInfo,
    non_inlinable: &std::collections::HashSet<String>,
) -> ModuleAnalysis {
    let audit_start = Instant::now();
    emit_module_stage_audit("start", module, None, audit_start.elapsed().as_millis());
    let call_graph = CallGraph::build(module);
    emit_module_stage_audit(
        "after-initial-call-graph",
        module,
        None,
        audit_start.elapsed().as_millis(),
    );
    let summaries = ModuleSummaries::compute(module, &call_graph);
    emit_module_stage_audit(
        "after-initial-summaries",
        module,
        None,
        audit_start.elapsed().as_millis(),
    );

    // E1: inline (a module transform — mutates bodies across functions).
    // `non_inlinable` names callees whose canonical definition is linked
    // externally (shared-stdlib-partition symbols the native/wasm driver will
    // externalize): the inliner refuses them so the app keeps the external
    // reference instead of forking a private copy of a body it does not own.
    let inline_stats =
        super::passes::inliner::run_inliner(module, &call_graph, &summaries, tti, non_inlinable);
    emit_module_stage_audit(
        "after-inliner",
        module,
        Some(inline_stats.changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );
    // Observability (mirrors TIR_OPT_STATS): the per-module inliner outcome, so
    // an unexpectedly-inert activation is visible instead of silently zero.
    if std::env::var("MOLT_INLINE_STATS").as_deref() == Ok("1") {
        eprintln!(
            "[E1] module '{}': {} call sites inlined into {} functions {:?}",
            module.name,
            inline_stats.sites_inlined,
            inline_stats.functions_changed,
            inline_stats.changed_functions,
        );
    }

    // Tier-B generator frame elision (doc 26 Phase 1 / D1 `07_D1-coroelide.md`).
    // Runs AFTER the inliner (a fused-away callee may have been inlined into the
    // poll body, and inlining cannot fire on the generator object itself) and
    // BEFORE module-slot-promotion (the fused loop's now-SSA frame slots are
    // ordinary loop phis that the promotion / value-range machinery refines).
    // The pass re-runs the per-function pipeline on each fused caller itself, so
    // its changed set is folded into `changed_functions` for the native
    // back-conversion + the LLVM/WASM direct lowering.
    let fusion_stats =
        super::passes::generator_fusion::run_generator_fusion(module, &call_graph, tti);
    emit_module_stage_audit(
        "after-generator-fusion",
        module,
        Some(fusion_stats.changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );
    if std::env::var("MOLT_INLINE_STATS").as_deref() == Ok("1") {
        eprintln!(
            "[Tier-B] module '{}': generator fusion elided {} frame(s), spliced {} yield site(s)",
            module.name, fusion_stats.frames_elided, fusion_stats.yield_sites_spliced,
        );
    }

    // Module-slot promotion (after inlining, so merged bodies — whose calls
    // disappeared — become promotable loops). Promoted functions are
    // re-optimized through the same refine→pipeline→refine contract the inliner
    // uses, so the value-range/RawI64Safe machinery proves the now-SSA loop
    // phis and the backends receive fully-refined bodies.
    let (promo_stats, promo_changed) =
        super::passes::module_slot_promotion::run_module_slot_promotion(module);
    emit_module_stage_audit(
        "after-module-slot-promotion",
        module,
        Some(promo_changed.len()),
        audit_start.elapsed().as_millis(),
    );
    if std::env::var("MOLT_INLINE_STATS").as_deref() == Ok("1") {
        eprintln!(
            "[E1] module '{}': slot-promotion {} slots / {} ops eliminated in {} functions {:?}",
            module.name,
            promo_stats.slots_promoted,
            promo_stats.ops_eliminated,
            promo_stats.functions_changed,
            promo_changed,
        );
    }
    if !promo_changed.is_empty() {
        let changed_set: std::collections::HashSet<&str> =
            promo_changed.iter().map(String::as_str).collect();
        for func in &mut module.functions {
            if changed_set.contains(func.name.as_str()) {
                super::type_refine::refine_types(func);
                let _ = super::passes::run_pipeline(func, tti);
                super::type_refine::refine_types(func);
            }
        }
    }
    emit_module_stage_audit(
        "after-module-slot-promotion-reopt",
        module,
        Some(promo_changed.len()),
        audit_start.elapsed().as_millis(),
    );

    let mut changed_functions = inline_stats.changed_functions;
    for name in fusion_stats.changed_functions {
        if !changed_functions.contains(&name) {
            changed_functions.push(name);
        }
    }
    for name in promo_changed {
        if !changed_functions.contains(&name) {
            changed_functions.push(name);
        }
    }

    // ── RC drop insertion: the terminal phase (design 20, round-7) ──────────
    //
    // Drop insertion runs HERE — once per function, AFTER the E1 inliner and
    // module-slot promotion (and the per-caller / per-promoted re-optimizations
    // those ran through the per-function pipeline) — rather than embedded in that
    // per-function pipeline, so it cannot defeat `module_slot_promotion` (a
    // refcount barrier in a loop blocks promotion of a module-global accumulator).
    // See [`crate::tir::drop_phase`] for the full structural rationale. Backend
    // conditioning lives inside the passes (`build_drop_pipeline` →
    // `target_uses_tir_drop_insertion`): on a non-activated target the phase is a
    // no-op, so this finalizer changes nothing there. The phase topology is
    // identical across every backend (the point of round-7).
    //
    // A function whose drop phase changed either op layout (inserted
    // `DecRef`/`IncRef`) or marker-only ownership facts (`drop_inserted`, which
    // native codegen reads to suppress its competing automatic temp-RC) is added
    // to `changed_functions` so the native/wasm drivers back-convert it to
    // SimpleIR; the LLVM lane lowers the whole module directly and ignores the
    // flag. A function with no drop-phase fact or op changes reports unchanged
    // and is left out — no wasted back-conversion.
    emit_module_stage_audit(
        "before-drop-finalize",
        module,
        Some(changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );
    let drop_changed = super::drop_phase::finalize_module_drops(module, tti);
    emit_module_stage_audit(
        "after-drop-finalize",
        module,
        Some(drop_changed.len()),
        audit_start.elapsed().as_millis(),
    );
    for name in drop_changed {
        if !changed_functions.contains(&name) {
            changed_functions.push(name);
        }
    }
    emit_module_stage_audit(
        "after-drop-change-merge",
        module,
        Some(changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );

    // Rebuild over the post-inline module: inlining removed `Call` ops and grew
    // caller bodies, so the leaf set / edges / op counts the returned analysis
    // exposes must reflect the merged program.
    emit_module_stage_audit(
        "before-post-call-graph",
        module,
        Some(changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );
    let call_graph = CallGraph::build(module);
    emit_module_stage_audit(
        "after-post-call-graph",
        module,
        Some(changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );
    let summaries = ModuleSummaries::compute(module, &call_graph);
    emit_module_stage_audit(
        "after-post-summaries",
        module,
        Some(changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );

    // CallFacts (foundation design 47): the per-call-site fact records, built
    // over the SAME post-inline module + call graph + summaries the backends
    // lower. Pure analysis (read-only — it records facts onto a side-table keyed
    // by each call op's result `ValueId`, it never mutates a body), so it is
    // byte-identical: Phase 1a attaches the facts but nothing on the hot path
    // consumes them yet. `is_inlineable`'s own eligibility classifier fills the
    // `inlinable` field, so the recorded facts can never disagree with the
    // inliner (single source of truth, doc 47 §7).
    emit_module_stage_audit(
        "before-call-facts",
        module,
        Some(changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );
    let call_facts =
        super::call_facts::CallFactsTable::build_module(module, &call_graph, &summaries, tti);
    emit_module_stage_audit(
        "after-call-facts",
        module,
        Some(changed_functions.len()),
        audit_start.elapsed().as_millis(),
    );

    // S5-1.5 producer-effect instrument: how many functions now have a
    // MemGVN-forwardable typed-slot load pair (two loads with the same reaching
    // memory version, object root, and field offset). Gated on
    // `MOLT_MEMGVN_REPORT=1` so it never costs a production build.
    if std::env::var("MOLT_MEMGVN_REPORT").as_deref() == Ok("1") {
        report_memgvn_producer_effect(module);
    }

    ModuleAnalysis {
        call_graph,
        summaries,
        changed_functions,
        call_facts,
    }
}

/// Count, per module, how many functions have at least one MemGVN-forwardable
/// typed-slot load PAIR: two `LoadAttr` uses sharing the same reaching memory
/// version, the same object root (alias-canonicalized), and the same field
/// offset. Such a pair is exactly what MemGVN (S5-2b) would dedup — so the count
/// is the direct producer effect of the class-aware `TypedField` regions.
///
/// Also reports the denominator (functions with ≥2 typed-slot loads at all) so a
/// zero numerator is diagnosable. Writes to stderr (the `MOLT_INLINE_STATS`
/// channel convention).
fn report_memgvn_producer_effect(module: &TirModule) {
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrValue, OpCode};
    use crate::tir::passes::alias_analysis::{AliasAnalysisResult, MemRegion};
    use crate::tir::passes::memory_ssa::{MemAccess, compute_standalone};

    // BASELINE mode (`MOLT_MEMGVN_REPORT_BASELINE=1`): strip the `_class` attr
    // from every op before analysis, so every typed-slot field op fails closed
    // to `GenericHeap` — exactly the pre-S5-1.5 behavior. Running the report once
    // with and once without this flag, over the SAME frontend IR, isolates the
    // producer effect of the class-aware regions (controlling for any frontend
    // version skew).
    let baseline = std::env::var("MOLT_MEMGVN_REPORT_BASELINE").as_deref() == Ok("1");
    let strip_class = |func: &TirFunction| -> TirFunction {
        if !baseline {
            return func.clone();
        }
        let mut f = func.clone();
        for block in f.blocks.values_mut() {
            for op in &mut block.ops {
                op.attrs.remove("_class");
            }
        }
        f
    };

    let mut funcs_with_two_loads = 0usize;
    let mut funcs_with_forwardable_pair = 0usize;
    let mut total_forwardable_pairs = 0usize;
    // The backend runs as a daemon whose stderr does not surface through the CLI
    // on a successful build, so the per-function lines are collected and written
    // to the debug-artifact channel as well (the module_slot_promotion pattern).
    let mut report_lines: Vec<String> = Vec::new();

    // Raw diagnostics (denominator sanity): how many typed-slot LoadAttr ops
    // exist at all, and how many carry a `_class` attr / classify as TypedField.
    {
        let mut raw_loads = 0usize;
        let mut raw_loads_with_class = 0usize;
        let mut raw_typedfield_loads = 0usize;
        for func in &module.functions {
            let func = &strip_class(func);
            let alias = AliasAnalysisResult::compute(func);
            for block in func.blocks.values() {
                for op in &block.ops {
                    if op.opcode != OpCode::LoadAttr {
                        continue;
                    }
                    let kind = match op.attrs.get("_original_kind") {
                        Some(AttrValue::Str(s)) => s.as_str(),
                        _ => "",
                    };
                    if !matches!(kind, "load" | "guarded_field_get") {
                        continue;
                    }
                    raw_loads += 1;
                    if matches!(op.attrs.get("_class"), Some(AttrValue::Str(_))) {
                        raw_loads_with_class += 1;
                    }
                    if matches!(
                        alias.region_of(op),
                        crate::tir::passes::alias_analysis::MemRegion::TypedField { .. }
                            | crate::tir::passes::alias_analysis::MemRegion::StackObject { .. }
                    ) {
                        raw_typedfield_loads += 1;
                    }
                }
            }
        }
        let line = format!(
            "[S5-1.5] module '{}': raw typed-slot loads={raw_loads} (with _class={raw_loads_with_class}, classify TypedField/StackObject={raw_typedfield_loads})",
            module.name
        );
        eprintln!("{line}");
        report_lines.push(line);
    }

    for func in &module.functions {
        let func = &strip_class(func);
        let alias = AliasAnalysisResult::compute(func);
        let mem = compute_standalone(func, &alias);

        // Key a typed-slot load by (reaching def version, object root, field
        // offset). Two loads with the same key are a forwardable pair (MemGVN
        // would dedup the second into a copy of the first). Only loads whose
        // region is precise (TypedField / StackObject — i.e. class-aware) can
        // form such a pair; a GenericHeap load is clobbered by any preceding
        // GenericHeap def and never shares a precise version.
        let mut keyed: std::collections::HashMap<(u32, u32, i64), usize> =
            std::collections::HashMap::new();
        let mut typed_load_count = 0usize;

        for (&(block, op_idx), access) in &mem.uses {
            let MemAccess::Use {
                def_ver, region, ..
            } = access
            else {
                continue;
            };
            if !matches!(
                region,
                MemRegion::TypedField { .. } | MemRegion::StackObject { .. }
            ) {
                continue;
            }
            let op = &func.blocks[&block].ops[op_idx];
            if op.opcode != OpCode::LoadAttr {
                continue;
            }
            let Some(&obj) = op.operands.first() else {
                continue;
            };
            let root = alias.root(obj);
            let offset = match op.attrs.get("value") {
                Some(AttrValue::Int(v)) => *v,
                _ => continue,
            };
            typed_load_count += 1;
            *keyed.entry((def_ver.0, root.0, offset)).or_insert(0) += 1;
        }

        if typed_load_count >= 2 {
            funcs_with_two_loads += 1;
        }
        let pairs: usize = keyed.values().filter(|&&c| c >= 2).map(|&c| c - 1).sum();
        if pairs > 0 {
            funcs_with_forwardable_pair += 1;
            total_forwardable_pairs += pairs;
            let line = format!(
                "[S5-1.5] {}::{} — {} forwardable typed-slot load pair(s)",
                module.name, func.name, pairs
            );
            eprintln!("{line}");
            report_lines.push(line);
        }
    }

    let summary = format!(
        "[S5-1.5] module '{}': {}/{} functions (with >=2 typed-slot loads) have a MemGVN-forwardable typed-slot load pair ({} pairs total)",
        module.name, funcs_with_forwardable_pair, funcs_with_two_loads, total_forwardable_pairs
    );
    eprintln!("{summary}");
    report_lines.push(summary);

    let sanitized: String = module
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
    let _ = crate::debug_artifacts::write_debug_artifact(
        format!("memgvn_report/{sanitized}.txt"),
        report_lines.join("\n") + "\n",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    fn func_calling(name: &str, callees: &[&str]) -> TirFunction {
        let mut func = TirFunction::new(name.into(), vec![], TirType::None);
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).unwrap();
        for callee in callees {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str((*callee).to_string()));
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![],
                attrs,
                source_span: None,
            });
        }
        block.terminator = Terminator::Return { values: vec![] };
        func
    }

    fn module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "m".into(),
            functions: funcs,
        }
    }

    #[test]
    fn module_phase_produces_call_graph_and_summaries() {
        // `b` is a trivial inlinable leaf (no ops, just Return), so the module
        // phase inlines the static call `a → b`. The returned analysis is
        // rebuilt over the post-inline module: every function is summarized, and
        // because a's only call was to the inlined-away b, the post-inline graph
        // records no a→b edge (a is now a leaf).
        let mut m = module(vec![func_calling("a", &["b"]), func_calling("b", &[])]);
        let tti = TargetInfo::native_release_fast();
        let analysis = run_module_pipeline(&mut m, &tti, &std::collections::HashSet::new());

        // Both functions remain summarized (the post-inline rebuild covers all).
        assert!(analysis.summaries.get("a").is_some());
        assert!(analysis.summaries.get("b").is_some());
        // b was inlined into a, so a no longer statically calls b.
        assert!(
            analysis.call_graph.callees("a").is_empty(),
            "a's static call to b was inlined away"
        );
        // After inlining, a makes no call → it joins the leaf set alongside b.
        assert!(analysis.leaf_functions().contains("a"));
        assert!(analysis.leaf_functions().contains("b"));
    }

    #[test]
    fn module_phase_leaf_set_matches_call_graph() {
        let mut m = module(vec![
            func_calling("a", &["b"]),
            func_calling("b", &["c"]),
            func_calling("c", &[]),
            func_calling("d", &[]),
        ]);
        let tti = TargetInfo::native_release_fast();
        let analysis = run_module_pipeline(&mut m, &tti, &std::collections::HashSet::new());
        assert_eq!(
            analysis.leaf_functions(),
            analysis.call_graph.leaf_functions()
        );
        assert_eq!(
            analysis.summaries.leaf_functions(),
            analysis.leaf_functions()
        );
    }

    #[test]
    fn module_phase_inlines_static_calls() {
        // The module phase now runs the E1 inliner (a body transform). A static
        // call `a → b` to an in-budget, exception-free, non-recursive leaf `b`
        // is inlined: a's body no longer contains the `Call` to b, and the
        // post-inline analysis no longer records the a→b edge.
        let a = func_calling("a", &["b"]);
        // `b` is a trivial leaf: a single ConstNone op + Return. Inlinable.
        let mut b = TirFunction::new("b".into(), vec![], TirType::None);
        let bentry = b.entry_block;
        let v = b.fresh_value();
        b.blocks.get_mut(&bentry).unwrap().ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v],
            attrs: AttrDict::new(),
            source_span: None,
        });
        b.blocks.get_mut(&bentry).unwrap().terminator = Terminator::Return { values: vec![] };

        let mut m = module(vec![a, b]);
        let tti = TargetInfo::native_release_fast();
        let _ = run_module_pipeline(&mut m, &tti, &std::collections::HashSet::new());

        // a's body no longer has a Call op (b was inlined).
        let a_after = m.functions.iter().find(|f| f.name == "a").unwrap();
        let a_calls: usize = a_after
            .blocks
            .values()
            .flat_map(|blk| blk.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(a_calls, 0, "the static call a→b is inlined away");

        // a is valid SSA after the merge + per-function re-optimization.
        crate::tir::verify::verify_function(a_after)
            .unwrap_or_else(|e| panic!("a invalid after module-phase inlining: {e:?}"));
    }
}
