use crate::SimpleIR;
use crate::wasm_plan::{emit_wasm_stage_audit, simple_ir_stage_shape, tir_module_stage_shape};

pub(super) fn run_tir_pipeline(ir: &mut SimpleIR) {
    emit_wasm_stage_audit(
        "compile-start",
        simple_ir_stage_shape(&ir.functions),
        None,
        None,
        None,
        None,
    );
    // ── TIR optimization pipeline ──
    // TIR is mandatory for backend-facing functions; bypassing it would
    // let SimpleIR transport metadata become a hidden representation
    // authority.
    let optimized_tir_by_name = {
        let tir_dump = crate::env_setting("TIR_DUMP").as_deref() == Some("1");
        let tir_stats = crate::env_setting("TIR_OPT_STATS").as_deref() == Some("1");
        let run = crate::tir::pipeline_cache::run_cached_tir_pipeline(
            &mut ir.functions,
            crate::tir::pipeline_cache::TirPipelineRunOptions {
                target_info: crate::tir::target_info::TargetInfo::wasm_release_fast(),
                cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Wasm,
                cache_dir: None,
                process_externs: false,
                verify_lir: false,
                tir_dump,
                tir_stats,
                progress_prefix: Some("MOLT_WASM"),
                resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan(),
            },
            |_| {},
        );
        emit_wasm_stage_audit(
            "after-function-pipeline",
            simple_ir_stage_shape(&ir.functions),
            None,
            None,
            None,
            None,
        );
        run.optimized_tir_by_name
    };
    let mut optimized_tir_by_name = optimized_tir_by_name;

    // E1 ACTIVATION (WASM): the TIR function inliner (tir/passes/inliner.rs,
    // via run_module_pipeline) is the production inliner — SSA-based,
    // exception-label-safe, call-graph bottom-up, cost-model-gated; it
    // re-optimizes each merged caller through the per-function pipeline.
    // Mirrors the native path: consume the shared cache runner's optimized TIR
    // custody for every non-extern function, run the module phase, then
    // back-convert ONLY the inliner-changed functions (every unchanged
    // function keeps its byte-identical per-function output). The legacy
    // SimpleIR `inline_functions` (string-rename, no SSA, no cost model) is
    // deleted with this activation. Rollback: MOLT_DISABLE_INLINING=1
    // (guard in run_inliner).
    {
        let wasm_tti = crate::tir::target_info::TargetInfo::wasm_release_fast();
        emit_wasm_stage_audit(
            "before-module-lower",
            simple_ir_stage_shape(&ir.functions),
            None,
            None,
            None,
            None,
        );
        let mut tir_functions = Vec::new();
        let mut idx_map = Vec::new();
        for (idx, func_ir) in ir.functions.iter().enumerate() {
            if func_ir.is_extern {
                continue;
            }
            let tir_func = optimized_tir_by_name
                .remove(&func_ir.name)
                .unwrap_or_else(|| {
                    panic!(
                        "WASM TIR cache runner did not return optimized TIR for '{}'",
                        func_ir.name
                    )
                });
            tir_functions.push(tir_func);
            idx_map.push(idx);
        }
        let mut tir_module = crate::tir::function::TirModule {
            name: "wasm_module".to_string(),
            functions: tir_functions,
        };
        emit_wasm_stage_audit(
            "after-module-lower",
            tir_module_stage_shape(&tir_module),
            None,
            None,
            None,
            None,
        );
        // WASM links the whole program into one module — there is no
        // shared-stdlib external partition, so every body is locally owned
        // and the inliner is unconstrained (empty external-linkage set).
        let non_inlinable = std::collections::HashSet::new();
        let module_pipeline_start = std::time::Instant::now();
        let module_analysis =
            crate::tir::run_module_pipeline(&mut tir_module, &wasm_tti, &non_inlinable);
        emit_wasm_stage_audit(
            "after-module-pipeline",
            tir_module_stage_shape(&tir_module),
            None,
            None,
            Some(module_analysis.changed_functions.len()),
            Some(module_pipeline_start.elapsed().as_millis()),
        );
        let changed: std::collections::HashSet<&str> = module_analysis
            .changed_functions
            .iter()
            .map(String::as_str)
            .collect();
        for (pos, &orig_idx) in idx_map.iter().enumerate() {
            let tir_func = &tir_module.functions[pos];
            if !changed.contains(tir_func.name.as_str()) {
                continue;
            }
            let ops = crate::tir::lower_to_simple::lower_to_simple_ir(tir_func);
            debug_assert!(
                crate::tir::lower_to_simple::validate_labels(&ops),
                "E1: inlined back-conversion emitted invalid labels for '{}' (WASM)",
                tir_func.name
            );
            ir.functions[orig_idx].ops = ops;
        }
        emit_wasm_stage_audit(
            "after-module-backconvert",
            simple_ir_stage_shape(&ir.functions),
            None,
            None,
            Some(changed.len()),
            None,
        );
    }
}
