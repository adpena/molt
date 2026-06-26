use crate::SimpleIR;
use crate::wasm_plan::{
    emit_wasm_stage_audit, is_production_lir_wasm_fast_path_name, prepare_lir_wasm_fast_output,
    simple_ir_stage_shape, tir_module_stage_shape,
};
use std::collections::BTreeMap;

pub(super) fn run_tir_pipeline(
    ir: &mut SimpleIR,
    lir_fast_outputs: &mut BTreeMap<String, crate::tir::lower_to_wasm::WasmFunctionOutput>,
) {
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
    {
        let tir_dump = crate::env_setting("TIR_DUMP").as_deref() == Some("1");
        let tir_stats = crate::env_setting("TIR_OPT_STATS").as_deref() == Some("1");
        let mut tir_cache =
            crate::tir::cache::CompilationCache::open(crate::tir::cache::backend_cache_dir());
        for func_ir in &mut ir.functions {
            // Compute a stable content hash from the function name + input ops.
            let body_bytes = crate::tir::serialize::serialize_ops(&func_ir.ops);
            let content_hash = crate::tir::cache::CompilationCache::compute_hash_with_signature(
                &func_ir.name,
                &func_ir.params,
                func_ir.param_types.as_deref(),
                &body_bytes,
            );

            // Cache hit: restore previously optimized ops and skip the pipeline.
            if let Some(cached_bytes) = tir_cache.get(&content_hash)
                && let Some(cached_ops) = crate::tir::serialize::deserialize_ops(&cached_bytes)
            {
                func_ir.ops = cached_ops;
                let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(func_ir);
                crate::tir::type_refine::refine_types(&mut tir_func);
                if is_production_lir_wasm_fast_path_name(&func_ir.name)
                    && let Some(output) = prepare_lir_wasm_fast_output(&tir_func)
                {
                    lir_fast_outputs.insert(func_ir.name.clone(), output);
                }
                continue;
            }

            let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(func_ir);
            crate::tir::type_refine::refine_types(&mut tir_func);
            let stats = crate::tir::passes::run_pipeline(
                &mut tir_func,
                &crate::tir::target_info::TargetInfo::wasm_release_fast(),
            );
            crate::tir::type_refine::refine_types(&mut tir_func);
            if tir_dump {
                eprintln!("{}", crate::tir::printer::print_function(&tir_func));
            }
            if tir_stats {
                for s in &stats {
                    eprintln!(
                        "[TIR] {}: {} values changed, {} attrs changed, {} removed, {} added",
                        s.name, s.values_changed, s.attrs_changed, s.ops_removed, s.ops_added
                    );
                }
            }
            let optimized_ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir_func);
            assert!(
                crate::tir::lower_to_simple::validate_labels(&optimized_ops),
                "TIR roundtrip emitted invalid labels for '{}' (WASM)",
                func_ir.name
            );
            let serialized = crate::tir::serialize::serialize_ops(&optimized_ops);
            tir_cache.put(&content_hash, &serialized, vec![]);
            func_ir.ops = optimized_ops;
            // Compute the LIR fast output from optimized TIR itself. The
            // value-keyed `repr_by_value` proof is pure TIR; SimpleIR
            // round-tripping is transport, not carrier authority.
            if is_production_lir_wasm_fast_path_name(&func_ir.name)
                && let Some(output) = prepare_lir_wasm_fast_output(&tir_func)
            {
                lir_fast_outputs.insert(func_ir.name.clone(), output);
            }
        }
        // Persist the updated cache index so future runs benefit.
        tir_cache.save_index();
        emit_wasm_stage_audit(
            "after-function-pipeline",
            simple_ir_stage_shape(&ir.functions),
            None,
            None,
            None,
            None,
        );
    }

    // E1 ACTIVATION (WASM): the TIR function inliner (tir/passes/inliner.rs,
    // via run_module_pipeline) is the production inliner — SSA-based,
    // exception-label-safe, call-graph bottom-up, cost-model-gated; it
    // re-optimizes each merged caller through the per-function pipeline.
    // Mirrors the native path: lift every non-extern function's
    // per-function-optimized SimpleIR to TIR, run the module phase, then
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
        let (mut tir_module, idx_map) =
            crate::tir::lower_from_simple::lower_functions_to_tir_module(&ir.functions);
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
            // The LIR fast-path output was computed per-function PRE-inline
            // (the cache loop above). An inlined-into allowlist function's
            // body changed, so recompute its output from the post-inline
            // TIR. A fast path that no longer applies is removed (the
            // generic emission path takes over - sound).
            let func_ir = &ir.functions[orig_idx];
            if is_production_lir_wasm_fast_path_name(&func_ir.name) {
                match prepare_lir_wasm_fast_output(tir_func) {
                    Some(output) => {
                        lir_fast_outputs.insert(func_ir.name.clone(), output);
                    }
                    None => {
                        lir_fast_outputs.remove(&func_ir.name);
                    }
                }
            }
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
