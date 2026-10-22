use crate::SimpleIR;
use crate::wasm_plan::{
    WasmStageAudit, emit_wasm_stage_audit, simple_ir_stage_shape, tir_functions_stage_shape,
};
use molt_ir::backend_environment::BackendEnv;

pub(super) fn run_tir_pipeline(
    ir: &mut SimpleIR,
    target_info: &crate::tir::target_info::TargetInfo,
    stage_audit: WasmStageAudit,
) -> Vec<(bool, crate::tir::TirFunction)> {
    emit_wasm_stage_audit(
        stage_audit,
        "compile-start",
        || simple_ir_stage_shape(&ir.functions),
        None,
        None,
        None,
        || None,
    );
    // TIR optimization pipeline.
    // TIR is mandatory for backend-facing functions; bypassing it would let
    // SimpleIR transport metadata become a hidden representation authority.
    let mut cached_tir = {
        let tir_dump = crate::env_setting(BackendEnv::TIR_DUMP).as_deref() == Some("1");
        let tir_stats = crate::env_setting(BackendEnv::TIR_OPT_STATS).as_deref() == Some("1");
        let run = crate::tir::pipeline_cache::run_cached_tir_pipeline(
            crate::tir::pipeline_cache::TirPipelineInput::SimpleIr(&mut ir.functions),
            crate::tir::pipeline_cache::TirPipelineRunOptions {
                target_info: target_info.clone(),
                cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Wasm,
                cache: crate::tir::pipeline_cache::TirCacheLocation::ProcessRoot,
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
            stage_audit,
            "after-function-pipeline",
            || simple_ir_stage_shape(&ir.functions),
            None,
            None,
            None,
            || None,
        );
        run.cached_tir
    };

    // Newly outlined transport enters the same ownership phase as its body.
    crate::tir::pipeline_cache::partition_cached_functions_before_drops(
        ir,
        &mut cached_tir,
        target_info,
        super::compile_pipeline::split_wasm_megafunctions,
    );

    // Consume the same retained owner as native/LLVM. Only the final owner
    // may project operations and facts after module transforms and drops.
    let non_inlinable = std::collections::HashSet::new();
    emit_wasm_stage_audit(
        stage_audit,
        "before-module-pipeline",
        || simple_ir_stage_shape(&ir.functions),
        None,
        None,
        None,
        || None,
    );
    let module_start = stage_audit.start();
    let typed = crate::tir::pipeline_cache::run_owned_module_pipeline_from_cached_tir(
        &ir.functions,
        &mut cached_tir,
        crate::tir::pipeline_cache::TirOwnedModulePipelineOptions {
            target_info,
            module_name: "wasm_module",
            non_inlinable: &non_inlinable,
            missing_tir_context: "WASM TIR cache runner",
            mode: crate::tir::pipeline_cache::TirOwnedModulePipelineMode::ModulePhase,
        },
    );
    emit_wasm_stage_audit(
        stage_audit,
        "after-module-pipeline",
        || {
            tir_functions_stage_shape(
                typed
                    .iter()
                    .filter_map(|(external, function)| (!*external).then_some(function)),
            )
        },
        None,
        None,
        None,
        || module_start.map(|start| start.elapsed().as_millis()),
    );
    typed
}
