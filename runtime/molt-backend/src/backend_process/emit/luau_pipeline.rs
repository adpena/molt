use molt_backend::SimpleIR;
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LuauTirModulePipelineStats {
    pub(crate) functions: usize,
    pub(crate) module_changed: usize,
}

pub(crate) fn run_luau_tir_module_pipeline(
    ir: &mut SimpleIR,
) -> io::Result<LuauTirModulePipelineStats> {
    let target_info = molt_backend::tir::target_info::TargetInfo::luau_release_fast();
    let local_function_count = ir.functions.iter().filter(|func| !func.is_extern).count();
    let mut tir_run = molt_backend::tir::pipeline_cache::run_cached_tir_pipeline(
        &mut ir.functions,
        molt_backend::tir::pipeline_cache::TirPipelineRunOptions {
            target_info: target_info.clone(),
            cache_flavor: molt_backend::tir::pipeline_cache::TirPipelineCacheFlavor::Luau,
            cache_dir: None,
            process_externs: false,
            verify_lir: false,
            tir_dump: std::env::var("TIR_DUMP").ok().as_deref() == Some("1"),
            tir_stats: std::env::var("TIR_OPT_STATS").ok().as_deref() == Some("1"),
            progress_prefix: None,
            resource_plan: molt_backend::tir::pipeline_cache::tir_optimization_resource_plan(),
        },
        |_| {},
    );
    let non_inlinable = std::collections::HashSet::new();
    let module_run =
        molt_backend::tir::pipeline_cache::run_simple_ir_module_pipeline_from_cached_tir(
            &mut ir.functions,
            &mut tir_run.cached_tir,
            molt_backend::tir::pipeline_cache::TirSimpleIrModulePipelineOptions {
                target_info: &target_info,
                module_name: "luau_module",
                non_inlinable: &non_inlinable,
                missing_tir_context: "Luau TIR cache runner",
                backconvert_context: "Luau TIR module pipeline",
                stage_observer: None,
            },
        );

    Ok(LuauTirModulePipelineStats {
        functions: local_function_count,
        module_changed: module_run.module_analysis.changed_functions.len(),
    })
}
