use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::super::{
    NativeApplicationObjectOptions, NativeBatchJobSpec, NativeBatchModuleMetadata,
    NativeBatchObjectJob, append_referenced_external_declarations, batch_external_function_names,
};
use super::plan::NativeApplicationBatchPlan;
use crate::backend_process::io_limits::write_json_artifact;

pub(super) fn materialize_native_application_batch_jobs(
    tmp_dir: &Path,
    plan: NativeApplicationBatchPlan,
    options: &mut NativeApplicationObjectOptions<'_>,
    batch_ops_budget: usize,
) -> io::Result<Vec<NativeBatchJobSpec>> {
    let module_context_path = tmp_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata {
            module_context: plan.module_context,
        },
    )?;
    let total_batches = plan.batches.len();
    let mut batch_specs: Vec<NativeBatchJobSpec> = Vec::new();
    for (batch_idx, batch_funcs) in plan.batches.into_iter().enumerate() {
        log_native_application_batch(
            options.log_prefix,
            batch_idx,
            total_batches,
            &batch_funcs,
            batch_ops_budget,
        );
        let mut batch_ir = SimpleIR {
            functions: batch_funcs,
            profile: plan.profile.clone(),
        };
        let external_function_names =
            batch_external_function_names(&plan.all_function_names, &batch_ir.functions);
        append_referenced_external_declarations(
            &mut batch_ir.functions,
            &plan.external_function_declarations,
        );
        let job_path = tmp_dir.join(format!("batch_{batch_idx}.json"));
        let batch_path = tmp_dir.join(format!("batch_{batch_idx}.o"));
        write_json_artifact(
            &job_path,
            &NativeBatchObjectJob {
                ir: batch_ir,
                module_context_path: module_context_path.clone(),
                target_triple: options.target_triple.map(str::to_owned),
                emit_app_callable_resolver: batch_idx == 0,
                app_callable_manifest: if batch_idx == 0 {
                    options.app_callable_manifest.take()
                } else {
                    None
                },
                external_function_names,
                // The resolver-emitting batch is the main application object;
                // it also owns the module registry blob. Init functions in
                // sibling batches resolve through Import relocations at link.
                module_registry: if batch_idx == 0 {
                    options.module_registry.take()
                } else {
                    None
                },
            },
        )?;
        batch_specs.push(NativeBatchJobSpec {
            job_path,
            object_path: batch_path,
        });
    }
    Ok(batch_specs)
}

fn log_native_application_batch(
    log_prefix: &str,
    batch_idx: usize,
    total_batches: usize,
    batch_funcs: &[molt_backend::FunctionIR],
    batch_ops_budget: usize,
) {
    let batch_ops = batch_funcs
        .iter()
        .fold(0usize, |ops, func| ops.saturating_add(func.ops.len()));
    eprintln!(
        "{log_prefix}: batch {}/{} ({} functions, {} ops / budget {})",
        batch_idx + 1,
        total_batches,
        batch_funcs.len(),
        batch_ops,
        batch_ops_budget
    );
}
