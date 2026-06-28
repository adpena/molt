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
    // TIR optimization pipeline.
    // TIR is mandatory for backend-facing functions; bypassing it would let
    // SimpleIR transport metadata become a hidden representation authority.
    let mut cached_tir = {
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
        run.cached_tir
    };

    // WASM links the whole program into one module: there is no shared-stdlib
    // external partition, so every body is locally owned and the inliner is
    // unconstrained. Cache custody, module assembly, module-phase execution,
    // selective back-conversion, and label validation all live in the shared
    // TIR pipeline authority.
    let wasm_tti = crate::tir::target_info::TargetInfo::wasm_release_fast();
    let non_inlinable = std::collections::HashSet::new();
    let mut stage_observer = emit_wasm_tir_pipeline_stage;
    let _module_run = crate::tir::pipeline_cache::run_simple_ir_module_pipeline_from_cached_tir(
        &mut ir.functions,
        &mut cached_tir,
        crate::tir::pipeline_cache::TirSimpleIrModulePipelineOptions {
            target_info: &wasm_tti,
            module_name: "wasm_module",
            non_inlinable: &non_inlinable,
            missing_tir_context: "WASM TIR cache runner",
            backconvert_context: "WASM TIR module pipeline",
            stage_observer: Some(&mut stage_observer),
        },
    );
}

fn emit_wasm_tir_pipeline_stage(
    stage: crate::tir::pipeline_cache::TirSimpleIrModulePipelineStage<'_>,
) {
    match stage {
        crate::tir::pipeline_cache::TirSimpleIrModulePipelineStage::BeforeModuleLower {
            functions,
        } => emit_wasm_stage_audit(
            "before-module-lower",
            simple_ir_stage_shape(functions),
            None,
            None,
            None,
            None,
        ),
        crate::tir::pipeline_cache::TirSimpleIrModulePipelineStage::AfterModuleLower { module } => {
            emit_wasm_stage_audit(
                "after-module-lower",
                tir_module_stage_shape(module),
                None,
                None,
                None,
                None,
            )
        }
        crate::tir::pipeline_cache::TirSimpleIrModulePipelineStage::AfterModulePipeline {
            module,
            changed_functions,
            elapsed_ms,
        } => emit_wasm_stage_audit(
            "after-module-pipeline",
            tir_module_stage_shape(module),
            None,
            None,
            Some(changed_functions),
            Some(elapsed_ms),
        ),
        crate::tir::pipeline_cache::TirSimpleIrModulePipelineStage::AfterModuleBackconvert {
            functions,
            changed_functions,
        } => emit_wasm_stage_audit(
            "after-module-backconvert",
            simple_ir_stage_shape(functions),
            None,
            None,
            Some(changed_functions),
            None,
        ),
    }
}
