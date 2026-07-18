use molt_backend::SimpleIR;
use molt_tir::ir_rewrites::rewrite_phi_to_store_load;
use serde_json::{Value, json};

use super::input::read_input;
use super::json_report::{aggregate_functions, function_stats_json};
use super::names::{repr_name, type_name};
use super::stats::collect_function_stats;

pub(crate) fn run() -> Result<(Value, bool), String> {
    let input = read_input()?;
    let mut ir: SimpleIR =
        serde_json::from_str(&input).map_err(|err| format!("invalid SimpleIR JSON: {err}"))?;
    let mut function_reports = Vec::with_capacity(ir.functions.len());
    let mut verified = true;
    for func in &mut ir.functions {
        if func.ops.iter().any(|op| op.kind == "phi") {
            rewrite_phi(func);
        }

        let mut tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(func);
        molt_backend::tir::type_refine::refine_types(&mut tir_func);
        let pass_stats = molt_backend::tir::passes::run_pipeline_with_translation_validation(
            &mut tir_func,
            &molt_backend::tir::target_info::TargetInfo::native_release_fast(),
        );
        molt_backend::tir::type_refine::refine_types(&mut tir_func);
        let lir_func =
            molt_backend::tir::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction(
                &tir_func,
            );

        let lir_errors = molt_backend::tir::verify_lir::verify_lir_function(&lir_func)
            .err()
            .unwrap_or_default();
        let repr_violations =
            molt_backend::tir::verify_lir_repr::verify_register_passable(&lir_func);
        if !lir_errors.is_empty() || !repr_violations.is_empty() {
            verified = false;
        }

        function_reports.push(json!({
            "name": lir_func.name,
            "blocks": lir_func.blocks.len(),
            "passes": pass_stats.iter().map(|stat| {
                json!({
                    "name": stat.name,
                    "values_changed": stat.values_changed,
                    "ops_removed": stat.ops_removed,
                    "ops_added": stat.ops_added,
                })
            }).collect::<Vec<_>>(),
            "stats": function_stats_json(&collect_function_stats(&lir_func)),
            "verification": {
                "lir_errors": lir_errors.iter().map(|err| format!("{err:?}")).collect::<Vec<_>>(),
                "repr_violations": repr_violations.iter().map(|violation| {
                    json!({
                        "block": violation.block.0,
                        "value": violation.value_id.0,
                        "expected_type": type_name(&violation.expected_type),
                        "expected_repr": repr_name(violation.expected_repr),
                        "actual_repr": repr_name(violation.actual_repr),
                    })
                }).collect::<Vec<_>>(),
            },
        }));
    }

    let aggregate = aggregate_functions(&function_reports);
    Ok((
        json!({
            "schema": "molt.typed_repr_report.v1",
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
