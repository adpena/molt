//! RC drop-insertion **terminal phase** (design 20, round-7).
//!
//! Drop insertion (`drop_insertion` + `refcount_elim_post`) is the compiler pass
//! family that closes molt's whole-program expression-temporary leak: the runtime
//! allocates every heap result at `ref_count = 1` and, before this family runs,
//! never decrements it for expression temporaries.
//!
//! ## Why this is a SEPARATE terminal phase (the round-7 structural arc)
//!
//! Drop insertion is deliberately NOT part of the per-function optimization
//! pipeline ([`crate::tir::passes::run_pipeline`] /
//! [`crate::tir::pass_manager::build_default_pipeline`]). It runs ONCE per
//! function, AFTER all per-function optimization AND all module-level transforms
//! (the E1 inliner + module-slot promotion, plus the per-caller / per-promoted
//! re-optimizations those run through the per-function pipeline).
//!
//! The reason is structural and load-bearing.
//! [`module_slot_promotion`](crate::tir::passes::module_slot_promotion) hoists a
//! module-global accumulator out of the module dict into a register-carried loop
//! phi (the `total = inc(total)` benchmark shape). Its soundness gate REFUSES to
//! promote a slot whose loop body carries a refcount barrier op
//! (`DecRef`/`IncRef`) — a finalizer running during the decrement could observe
//! the half-updated slot, so promoting across it is unsound. If drop insertion
//! ran inside the per-function pipeline (at step-1, or inside the inliner's
//! per-caller re-opt), it would seed those barrier ops into the loop BEFORE
//! promotion ran, and promotion would refuse every module-global accumulator —
//! leaving a per-iteration `module_get_attr`/`module_set_attr`/`dec_ref`
//! round-trip ~5× slower than the promoted register flow. Running drops as the
//! FINAL phase lets promotion see the clean loop and lets drops land on the final
//! (promoted) shape. This is design 20 §2.1's "runs LAST" made
//! whole-program-correct: after the module phase, not merely last within one
//! per-function pipeline invocation.
//!
//! ## The two entry shapes (one drop implementation)
//!
//! Both delegate to the single [`crate::tir::passes::run_drop_phase`]
//! implementation; they differ only in the IR carrier they iterate:
//!
//! * [`finalize_module_drops`] — runs over a [`TirModule`] in TIR form. Called by
//!   [`crate::tir::module_phase::run_module_pipeline`] as its terminal step (the
//!   whole-program build path: native non-batched, LLVM, WASM non-batched). The
//!   inliner already built the TIR module, so no SimpleIR round-trip is needed.
//! * [`finalize_simple_ir_drops`] — runs over a slice of `FunctionIR` (SimpleIR).
//!   Called by the `skip_ir_passes` build paths (the stdlib-cache object and the
//!   per-batch application codegen) where the whole-program module phase does NOT
//!   run, so the per-function pipeline is the last transform and drops must be
//!   applied after it, post-cache. It lifts each function to TIR, runs the drop
//!   phase, and back-converts every function whose executable ops or RC authority
//!   facts changed to SimpleIR.
//!
//! Both honor the same per-function invariants: full-function idempotency (the
//! drop pass bails on the `drop_inserted` marker, so a re-processed
//! fully-owned-RC function is a no-op), exception-only pre-bail idempotency (the
//! narrower `exception_region_drops_inserted` marker protects handler-safe
//! CreationRef/MatchRef releases without suppressing native legacy RC), and the
//! debug double-process guard (a function must not arrive already fully
//! drop-inserted).

use super::function::{TirFunction, TirModule};
use super::ops::AttrValue;
use super::passes::drop_insertion::DROP_INSERTED_ATTR;
use super::target_info::TargetInfo;
use std::time::Instant;

fn drop_stage_audit_enabled() -> bool {
    std::env::var("MOLT_DROP_STAGE_AUDIT").as_deref() == Ok("1")
        || std::env::var("MOLT_MODULE_STAGE_AUDIT").as_deref() == Ok("1")
        || std::env::var("MOLT_WASM_STAGE_AUDIT").as_deref() == Ok("1")
}

fn emit_drop_function_audit(
    stage: &str,
    func: &TirFunction,
    index: Option<usize>,
    total: Option<usize>,
    changed: Option<bool>,
    elapsed_ms: Option<u128>,
) {
    if !drop_stage_audit_enabled() {
        return;
    }
    if let Ok(filter) = std::env::var("MOLT_DROP_STAGE_AUDIT_FUNC")
        && !filter.trim().is_empty()
        && !func.name.contains(filter.trim())
    {
        return;
    }
    let blocks = func.blocks.len();
    let ops = func
        .blocks
        .values()
        .fold(0usize, |count, block| count.saturating_add(block.ops.len()));
    eprintln!(
        "[molt-drop-stage-audit] stage={stage} function={} index={} total={} blocks={} ops={} value_types={} attrs={} changed={} elapsed_ms={} peak_rss_mib={}",
        func.name,
        index
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        total
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        blocks,
        ops,
        func.value_types.len(),
        func.attrs.len(),
        changed
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        elapsed_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        crate::process_diagnostics::process_peak_rss_mib_label(),
    );
}

/// Run the RC drop phase on a single in-TIR function — the ONE per-function entry
/// every terminal-phase caller funnels through (the TIR-module finalizer, the
/// SimpleIR finalizer, and the LLVM `skip_ir_passes` branch). Returns `true` iff
/// the phase changed executable ops or semantic facts — i.e. the function now
/// carries `DecRef`/`IncRef` ops and/or one of the drop fact markers that must be
/// back-converted for SimpleIR consumers. `drop_inserted` is the full-function RC
/// authority marker that native codegen reads to suppress its competing
/// automatic temp-RC, so an attribute-only `drop_inserted` change counts even
/// when no physical `DecRef`/`IncRef` op was inserted.
/// `exception_region_drops_inserted` is only the handler-safe exception
/// transport slice and must not suppress native legacy RC. A function with no
/// droppable temporaries still reports `true` on activated targets when the
/// full-function marker is newly stamped; that marker is the backend authority
/// fact and must be preserved/back-converted.
///
/// `debug_assert`s that the function is not ALREADY drop-inserted on entry: the
/// only marker producers are this phase and the round-trip that preserves it, so
/// an already-marked function means drops were placed mid-transform (the bug
/// round-7 exists to prevent) or the finalizer ran twice. The drop pass is
/// idempotent (bails on the marker), so this is a debug-only invariant check, not
/// a correctness rail.
pub fn finalize_function_drops(func: &mut TirFunction, tti: &TargetInfo) -> bool {
    debug_assert!(
        !matches!(
            func.attrs.get(DROP_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ),
        "function '{}' is already drop-inserted before the terminal drop phase \
         — drops were placed mid-transform or the finalizer ran twice",
        func.name,
    );
    let audit_start = Instant::now();
    let had_drop_inserted = matches!(
        func.attrs.get(DROP_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    let had_exception_region_drops = matches!(
        func.attrs
            .get(super::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    // Drop placement keys on repr facts (`TirLivenessResult::is_raw_scalar`
    // distinguishes raw-i64/bool/float carriers — which hold no refcount and must
    // NOT be dropped — from heap-carrying values). Those facts are derived from
    // `value_types`, so the function MUST be type-refined before the drop pass
    // runs. When drops lived in the per-function pipeline this was guaranteed by
    // the `refine → run_pipeline → refine` bracket every backend wraps it in; now
    // that drops run as a separate terminal phase, functions the module phase did
    // NOT re-optimize (neither inlined nor promoted) arrive carrying only the
    // types their initial SimpleIR→TIR lift produced. Refining here makes the
    // invariant hold uniformly for every function (refinement is an idempotent
    // fixpoint, so re-refining the inlined/promoted bodies is a safe no-op).
    emit_drop_function_audit(
        "before-type-refine",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    super::type_refine::refine_types(func);
    emit_drop_function_audit(
        "after-type-refine",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    emit_drop_function_audit(
        "before-drop-pass",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    let stats = super::passes::run_drop_phase(func, tti);
    emit_drop_function_audit(
        "after-drop-pass",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    let changed: usize = stats
        .iter()
        .map(super::passes::PassStats::total_changes)
        .sum();
    let has_drop_inserted = matches!(
        func.attrs.get(DROP_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    let has_exception_region_drops = matches!(
        func.attrs
            .get(super::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    let changed = changed > 0
        || had_drop_inserted != has_drop_inserted
        || had_exception_region_drops != has_exception_region_drops;
    emit_drop_function_audit(
        "after-change-count",
        func,
        None,
        None,
        Some(changed),
        Some(audit_start.elapsed().as_millis()),
    );
    changed
}

/// Terminal drop phase over a [`TirModule`] in TIR form. Runs the drop phase on
/// every function and returns the names of the functions it changed (those the
/// caller must back-convert to SimpleIR; the LLVM lane lowers the module directly
/// and ignores the return). The order of the returned names follows
/// `module.functions`.
pub fn finalize_module_drops(module: &mut TirModule, tti: &TargetInfo) -> Vec<String> {
    let mut changed = Vec::new();
    let total = module.functions.len();
    let audit_start = Instant::now();
    for (index, func) in module.functions.iter_mut().enumerate() {
        emit_drop_function_audit(
            "before-function",
            func,
            Some(index),
            Some(total),
            None,
            Some(audit_start.elapsed().as_millis()),
        );
        let did_change = finalize_function_drops(func, tti);
        emit_drop_function_audit(
            "after-function",
            func,
            Some(index),
            Some(total),
            Some(did_change),
            Some(audit_start.elapsed().as_millis()),
        );
        if did_change {
            changed.push(func.name.clone());
        }
    }
    changed
}

/// Terminal drop phase over SimpleIR `FunctionIR` bodies (the `skip_ir_passes`
/// build paths: stdlib-cache object + per-batch application codegen). For each
/// non-extern function it lifts to TIR (type-refined), runs the drop phase, and —
/// if the phase changed the body — back-converts the drop-inserted TIR to
/// SimpleIR in place (which re-emits the drop fact marker ops). Functions the
/// phase did not change keep their existing (post-per-function-pipeline)
/// SimpleIR untouched.
///
/// Extern functions (shared-stdlib-partition symbols with cleared bodies) are
/// skipped: they have no body to drop and lifting one would fail the TIR
/// verifier — the same contract the per-function pipeline honors.
///
/// This is the post-cache step: it runs AFTER the (cached) per-function pipeline,
/// so the cache never stores drop-inserted ops keyed by the drop-free input hash.
pub fn finalize_simple_ir_drops(functions: &mut [crate::ir::FunctionIR], tti: &TargetInfo) {
    finalize_simple_ir_drops_with_tir_custody(
        functions,
        tti,
        &mut std::collections::BTreeMap::new(),
    );
}

/// Terminal drop phase over SimpleIR bodies, preferring already-optimized TIR
/// functions when the caller has them. This is the batch/native custody path:
/// per-function optimization produces typed TIR first, then SimpleIR only as a
/// legacy backend carrier. Running terminal drops from the TIR custody map avoids
/// re-lifting the expanded SimpleIR bridge form and keeps block-argument payloads
/// in TIR until the final backend lowering.
pub fn finalize_simple_ir_drops_with_tir_custody(
    functions: &mut [crate::ir::FunctionIR],
    tti: &TargetInfo,
    optimized_tir_by_name: &mut std::collections::BTreeMap<String, TirFunction>,
) {
    for func_ir in functions.iter_mut() {
        if func_ir.is_extern {
            continue;
        }
        let mut tir_func = optimized_tir_by_name
            .remove(&func_ir.name)
            .unwrap_or_else(|| super::lower_from_simple::lower_to_tir(func_ir));
        // `finalize_function_drops` refines types before the drop pass (the drop
        // placement needs repr facts), so no separate refinement is needed here.
        // The function arriving here already went through the per-function pipeline
        // (its SimpleIR is the optimized output), so it is NOT yet drop-inserted —
        // its debug_assert verifies that.
        if finalize_function_drops(&mut tir_func, tti) {
            let ops = super::lower_to_simple::lower_to_simple_ir(&tir_func);
            debug_assert!(
                super::lower_to_simple::validate_labels(&ops),
                "drop-phase back-conversion emitted invalid labels for '{}'",
                func_ir.name
            );
            func_ir.ops = ops;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::function::TirModule;

    /// A function with no droppable heap temporaries still reports changed on an
    /// activated target because the full-function `drop_inserted` authority fact
    /// must survive the pass-manager snapshot/restore boundary.
    #[test]
    fn module_finalizer_reports_marker_only_change_for_trivial_function() {
        // A trivial `return n` function: one param, returns it. No heap temps.
        let func_ir = FunctionIR {
            name: "trivial".into(),
            params: vec!["n".into()],
            ops: vec![OpIR {
                kind: "ret".into(),
                var: Some("n".into()),
                args: Some(vec!["n".into()]),
                ..OpIR::default()
            }],
            param_types: Some(vec!["Any".into()]),
            source_file: None,
            is_extern: false,
        };
        let mut tir = crate::tir::lower_from_simple::lower_to_tir(&func_ir);
        crate::tir::type_refine::refine_types(&mut tir);
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![tir],
        };
        let changed = finalize_module_drops(&mut module, &TargetInfo::native_release_fast());
        assert_eq!(changed, vec!["trivial"]);
        assert!(matches!(
            module.functions[0].attrs.get(DROP_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ));
        assert!(
            module.functions[0]
                .blocks
                .values()
                .flat_map(|block| block.ops.iter())
                .all(|op| op.opcode != super::super::ops::OpCode::DecRef
                    && op.opcode != super::super::ops::OpCode::IncRef),
            "borrowed param return needs no physical RC ops"
        );
    }

    /// Extern functions are skipped by the SimpleIR finalizer (no body to drop).
    #[test]
    fn simple_ir_finalizer_skips_extern() {
        let mut funcs = vec![FunctionIR {
            name: "ext".into(),
            params: vec![],
            ops: vec![],
            param_types: None,
            source_file: None,
            is_extern: true,
        }];
        // Must not panic (no lift of the empty extern body).
        finalize_simple_ir_drops(&mut funcs, &TargetInfo::native_release_fast());
        assert!(funcs[0].ops.is_empty());
    }

    #[test]
    fn simple_ir_finalizer_prefers_tir_custody() {
        let mut funcs = vec![FunctionIR {
            name: "custody".into(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }];

        let mut tir = TirFunction::new("custody".into(), vec![], crate::tir::types::TirType::I64);
        let value = tir.fresh_value();
        tir.value_types
            .insert(value, crate::tir::types::TirType::I64);
        tir.blocks.get_mut(&tir.entry_block).unwrap().ops = vec![crate::tir::ops::TirOp {
            dialect: crate::tir::ops::Dialect::Molt,
            opcode: crate::tir::ops::OpCode::ConstInt,
            operands: vec![],
            results: vec![value],
            attrs: {
                let mut attrs = crate::tir::ops::AttrDict::new();
                attrs.insert("value".into(), crate::tir::ops::AttrValue::Int(7));
                attrs
            },
            source_span: None,
        }];
        tir.blocks.get_mut(&tir.entry_block).unwrap().terminator =
            crate::tir::blocks::Terminator::Return {
                values: vec![value],
            };

        let mut custody = std::collections::BTreeMap::new();
        custody.insert("custody".into(), tir);
        finalize_simple_ir_drops_with_tir_custody(
            &mut funcs,
            &TargetInfo::native_release_fast(),
            &mut custody,
        );

        assert!(custody.is_empty());
        assert!(
            funcs[0]
                .ops
                .iter()
                .any(|op| op.kind == "const" && op.value == Some(7)),
            "finalizer must lower the TIR custody body, not the stale SimpleIR body"
        );
    }

    #[test]
    fn simple_ir_finalizer_back_converts_zero_drop_authority_marker() {
        let mut funcs = vec![FunctionIR {
            name: "borrowed_param_store".into(),
            params: vec!["self".into()],
            ops: vec![
                OpIR {
                    kind: "store_var".into(),
                    var: Some("self".into()),
                    args: Some(vec!["self".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["Any".into()]),
            source_file: None,
            is_extern: false,
        }];

        finalize_simple_ir_drops(&mut funcs, &TargetInfo::native_release_fast());

        assert_eq!(funcs[0].ops[0].kind, DROP_INSERTED_ATTR);
        assert!(
            funcs[0]
                .ops
                .iter()
                .all(|op| op.kind != "dec_ref" && op.kind != "inc_ref"),
            "borrowed-param-only SimpleIR must only gain the authority marker"
        );
    }

    #[test]
    fn native_roundtrip_preserves_call_bind_finalizer_fact_for_absorption_drops() {
        let func_ir = FunctionIR {
            name: "call_bind_finalizer_roundtrip".into(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_none".into(),
                    out: Some("cls".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".into(),
                    out: Some("callargs".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_bind".into(),
                    args: Some(vec!["cls".into(), "callargs".into()]),
                    out: Some("item".into()),
                    defines_del: Some(true),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_new".into(),
                    args: Some(vec!["item".into()]),
                    out: Some("bag".into()),
                    bound_local: Some(true),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("inside".into()),
                    out: Some("msg".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "warn_stderr".into(),
                    args: Some(vec!["msg".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let tti = TargetInfo::native_release_fast();
        let mut optimized_tir = crate::tir::lower_from_simple::lower_to_tir(&func_ir);
        crate::tir::type_refine::refine_types(&mut optimized_tir);
        crate::tir::passes::run_pipeline(&mut optimized_tir, &tti);
        crate::tir::type_refine::refine_types(&mut optimized_tir);
        let optimized_ops = crate::tir::lower_to_simple::lower_to_simple_ir(&optimized_tir);

        let roundtripped_call = optimized_ops
            .iter()
            .find(|op| op.kind == "call_bind")
            .expect("native per-function roundtrip must preserve call_bind");
        let item_name = roundtripped_call
            .out
            .clone()
            .expect("call_bind finalizer result must keep an output name");
        let bag_name = optimized_ops
            .iter()
            .find(|op| op.kind == "list_new")
            .and_then(|op| op.out.clone())
            .expect("absorbing list_new result must keep an output name");
        let roundtripped_list = optimized_ops
            .iter()
            .find(|op| op.kind == "list_new")
            .expect("absorbing list_new must survive native per-function roundtrip");
        assert_eq!(
            roundtripped_call.defines_del,
            Some(true),
            "defines_del is a result-lifetime fact and must survive native's \
             optimize-roundtrip before terminal drop insertion"
        );
        assert_eq!(
            roundtripped_list.bound_local,
            Some(true),
            "bound_local is the Python lifetime-boundary fact and must survive \
             native's optimize-roundtrip before terminal drop insertion"
        );

        let mut funcs = vec![FunctionIR {
            name: func_ir.name.clone(),
            params: func_ir.params.clone(),
            ops: optimized_ops,
            param_types: func_ir.param_types.clone(),
            source_file: None,
            is_extern: false,
        }];
        finalize_simple_ir_drops(&mut funcs, &tti);
        let ops = &funcs[0].ops;
        let warn_idx = ops
            .iter()
            .position(|op| op.kind == "warn_stderr")
            .expect("side effect must remain in the lowered body");
        let ret_idx = ops
            .iter()
            .position(|op| op.kind == "ret_void")
            .expect("function must still return");
        let list_idx = ops
            .iter()
            .position(|op| op.kind == "list_new")
            .expect("absorbing list constructor must remain in the lowered body");

        assert!(
            ops[list_idx + 1..warn_idx]
                .iter()
                .any(|op| op.kind == "dec_ref"
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == &item_name))),
            "drop insertion must release the absorbed call-owned item after \
             list_new takes ownership; ops={ops:?}"
        );
        assert!(
            ops[..warn_idx].iter().all(|op| {
                op.kind != "dec_ref"
                    || !op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == &bag_name))
            }),
            "the finalizer-sensitive container root must survive until after \
             later side effects; ops={ops:?}"
        );
        let post_warn_dec_refs: Vec<_> = ops[warn_idx + 1..ret_idx]
            .iter()
            .filter(|op| {
                op.kind == "dec_ref"
                    && op.args.as_ref().is_some_and(|args| {
                        args.iter().any(|arg| arg == &item_name || arg == &bag_name)
                    })
            })
            .collect();
        assert!(
            post_warn_dec_refs.iter().all(|op| {
                !op.args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|arg| arg == &item_name))
            }),
            "the absorbed item must not get a second terminal drop; ops={ops:?}"
        );
        assert!(
            post_warn_dec_refs.iter().any(|op| {
                op.args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|arg| arg == &bag_name))
            }),
            "terminal drop insertion must still release the finalizer-sensitive \
             container root before returning; ops={ops:?}"
        );
    }
}
