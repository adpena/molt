use std::io;
use std::path::Path;

use molt_backend::NativeBackendModuleContext;
use molt_ir::backend_environment::NativeCodegenEnvironment;

use super::super::super::{
    NativeApplicationArtifactOptions, NativeBatchInput, NativeBatchJobSpec, NativeBatchObjectJob,
    batch_external_function_names,
};
use super::plan::NativeApplicationBatchPlan;

pub(super) fn materialize_native_application_batch_jobs(
    tmp_dir: &Path,
    mut plan: NativeApplicationBatchPlan,
    options: &mut NativeApplicationArtifactOptions<'_>,
    codegen_environment: &NativeCodegenEnvironment,
    batch_ops_budget: usize,
) -> io::Result<Vec<NativeBatchJobSpec>> {
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
        let terminal = plan
            .archive
            .project(batch_funcs)
            .map_err(io::Error::other)?;
        let external_function_names = batch_external_function_names(&terminal.declarations);
        let batch_path = tmp_dir.join(format!("batch_{batch_idx}.o"));
        let job = NativeBatchObjectJob {
            ir: NativeBatchInput::Terminal { terminal },
            code_slots: (batch_idx == 0).then(|| std::mem::take(&mut options.code_slots)),
            module_context: NativeBackendModuleContext::default(),
            codegen_environment: codegen_environment.clone(),
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
        }
        .close_dependencies(&plan.archive.linkage);
        let input_key = options
            .object_cache
            .as_ref()
            .map(|cache| cache.input_key(&job))
            .transpose()?;
        batch_specs.push(NativeBatchJobSpec {
            input_key,
            job_name: format!("batch_{batch_idx}.msgpack"),
            job,
            object_path: batch_path,
        });
    }
    Ok(batch_specs)
}

fn log_native_application_batch(
    log_prefix: &str,
    batch_idx: usize,
    total_batches: usize,
    batch_funcs: &[molt_backend::TerminalObjectRow],
    batch_ops_budget: usize,
) {
    let batch_ops = batch_funcs
        .iter()
        .fold(0usize, |ops, func| ops.saturating_add(func.operations));
    eprintln!(
        "{log_prefix}: batch {}/{} ({} functions, {} ops / budget {})",
        batch_idx + 1,
        total_batches,
        batch_funcs.len(),
        batch_ops,
        batch_ops_budget
    );
}
