//! Whole-program transformations over optimized TIR.
//!
//! The shared drivers run this phase after per-function optimization. It builds
//! the call graph and summaries needed by inlining and generator fusion, then
//! promotes slots, re-optimizes changed bodies, and finalizes terminal drops.
//! The result names every changed function so SimpleIR consumers back-convert
//! only those bodies. Owned-TIR consumers lower the transformed module directly.
//!
//! Analyses of the final module belong to their consumers: FactGraph requests
//! precise call facts for its selected function, and native codegen computes its
//! leaf set after its later lifetime and function-splitting transformations.

use super::call_graph::CallGraph;
use super::function::TirModule;
use super::passes::ip_summary::ModuleSummaries;
use super::target_info::TargetInfo;
use std::collections::BTreeSet;
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
    molt_ir::stage_audit::stage_audit_enabled(molt_ir::stage_audit::AuditStage::Module)
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

/// Transform `module` and return changed function names in deterministic order.
/// Bodies and ownership markers can both require SimpleIR back-conversion.
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
) -> BTreeSet<String> {
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

    let mut changed_functions: BTreeSet<String> =
        inline_stats.changed_functions.into_iter().collect();
    changed_functions.extend(fusion_stats.changed_functions);
    changed_functions.extend(promo_changed);

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
    changed_functions.extend(drop_changed);
    emit_module_stage_audit(
        "after-drop-change-merge",
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

    changed_functions
}

/// Count, per module, how many functions have at least one MemGVN-forwardable
/// typed-slot load PAIR: two `LoadAttr` uses sharing the same reaching memory
/// version, the same object root (alias-canonicalized), and the same field
/// offset. Such a pair is exactly what MemGVN (S5-2b) would dedup — so the count
/// is the direct producer effect of physical field regions.
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
    // exist at all, and how many carry a `_class` attr / physical field region.
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
                        crate::tir::passes::alias_analysis::MemRegion::Field { .. }
                    ) {
                        raw_typedfield_loads += 1;
                    }
                }
            }
        }
        let line = format!(
            "[S5-1.5] module '{}': raw typed-slot loads={raw_loads} (with _class={raw_loads_with_class}, physical field regions={raw_typedfield_loads})",
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
        // region is a physical Field can
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
            if !matches!(region, MemRegion::Field { .. }) {
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
        let mut func = TirFunction::new(
            name.into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
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
    fn module_phase_inlines_static_calls() {
        // The module phase now runs the E1 inliner (a body transform). A static
        // call `a → b` to an in-budget, exception-free, non-recursive leaf `b`
        // is inlined: a's body no longer contains the `Call` to b.
        let a = func_calling("a", &["b"]);
        // `b` is a trivial leaf: a single ConstNone op + Return. Inlinable.
        let mut b = TirFunction::new(
            "b".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
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
        let changed = run_module_pipeline(&mut m, &tti, &std::collections::HashSet::new());
        // Inlining changes a; terminal ownership markers change both a and b.
        // Each name appears once even when multiple phases change the body.
        assert_eq!(changed, BTreeSet::from(["a".into(), "b".into()]));
        assert_eq!(
            m.functions.len(),
            2,
            "inlining preserves function definitions"
        );

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
