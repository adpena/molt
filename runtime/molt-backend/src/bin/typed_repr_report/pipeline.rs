use molt_backend::SimpleIR;
use molt_tir::ir_rewrites::rewrite_phi_to_store_load;
use serde_json::{Value, json};

use super::input::read_input;
use super::json_report::{aggregate_functions, function_stats_json};
use super::stats::collect_function_stats;

pub(crate) fn run() -> Result<(Value, bool), String> {
    let input = read_input()?;
    let mut ir: SimpleIR =
        serde_json::from_str(&input).map_err(|err| format!("invalid SimpleIR JSON: {err}"))?;
    let mut function_reports = Vec::with_capacity(ir.functions.len());
    let mut verified = true;
    for func in &mut ir.functions {
        if func.ops.iter().any(|op| op.kind == molt_ir::OpKind::Phi) {
            rewrite_phi(func);
        }

        // The production sequence (pipeline_cache::optimize_tir_input), so the
        // counts are the carriers a native build's backend receives.
        let target = molt_backend::tir::target_info::TargetInfo::native_release_fast();
        let mut tir_func =
            molt_backend::tir::lower_from_simple::lower_to_tir_for_target(func, &target);
        molt_backend::tir::type_refine::refine_types(&mut tir_func);
        let pass_stats = molt_backend::tir::passes::run_pipeline(&mut tir_func, &target);
        molt_backend::tir::type_refine::refine_types(&mut tir_func);
        let ranges = molt_backend::representation_plan::value_range_for(&tir_func);
        let repr = molt_backend::representation_plan::repr_by_value_for(&tir_func, Some(&ranges));
        let tir_errors = molt_backend::tir::verify::verify_function(&tir_func)
            .err()
            .unwrap_or_default();
        if !tir_errors.is_empty() {
            verified = false;
        }

        function_reports.push(json!({
            "name": tir_func.name,
            "blocks": tir_func.blocks.len(),
            "passes": pass_stats.iter().map(|stat| {
                json!({
                    "name": stat.name,
                    "values_changed": stat.values_changed,
                    "ops_removed": stat.ops_removed,
                    "ops_added": stat.ops_added,
                })
            }).collect::<Vec<_>>(),
            "stats": function_stats_json(&collect_function_stats(&tir_func, &repr)),
            "verification": {
                "tir_errors": tir_errors.iter().map(|err| format!("{err:?}")).collect::<Vec<_>>(),
            },
        }));
    }

    let aggregate = aggregate_functions(&function_reports);
    Ok((
        json!({
            "schema": "molt.typed_repr_report.v2",
            "verified": verified,
            "functions": function_reports,
            "aggregate": aggregate,
        }),
        verified,
    ))
}

fn rewrite_phi(func: &mut molt_backend::FunctionIR) {
    rewrite_phi_to_store_load(&mut func.ops);
}
