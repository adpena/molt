mod jobs;
mod plan;
mod reuse;
mod run;

use std::io;
use std::path::Path;

use molt_backend::SimpleIR;
use molt_ir::backend_environment::NativeCodegenEnvironment;

use super::super::{NativeApplicationArtifactOptions, NativeApplicationArtifactResult};
use plan::NativeApplicationBatchPlan;

pub(crate) fn compile_batched_native_application_archive_to_path(
    ir: SimpleIR,
    output_path: &Path,
    options: &mut NativeApplicationArtifactOptions<'_>,
    function_count: usize,
    batch_size: usize,
    batch_ops_budget: usize,
) -> io::Result<NativeApplicationArtifactResult> {
    let environment = NativeCodegenEnvironment::capture()?;
    let mut plan_record = if let Some(cache) = options.object_cache.as_ref() {
        let mut record = reuse::ApplicationPlanRecord::new(
            cache,
            &ir,
            options,
            &environment,
            batch_size,
            batch_ops_budget,
        )?;
        if let Some(artifact) =
            record.reuse(cache, output_path, function_count, options.log_prefix)?
        {
            return Ok(artifact);
        }
        Some(record)
    } else {
        None
    };
    let mut backend = molt_backend::SimpleBackend::new_with_target(options.target_triple);
    backend.code_slots = Some(std::mem::take(&mut options.code_slots));
    backend.module_registry = options.module_registry.take();
    let mut plan = NativeApplicationBatchPlan::from_ir(
        ir,
        batch_size,
        batch_ops_budget,
        options.module_context.take(),
        options.source_facts.take(),
        options.app_callable_manifest.take(),
        &mut backend,
    )?;
    options.code_slots = backend
        .code_slots
        .take()
        .expect("application code slot bindings");
    options.module_registry = backend.module_registry.take();
    drop(backend);
    let total_batches = plan.total_batches();
    options.app_callable_manifest = Some(std::mem::take(&mut plan.archive.callables));

    let tmp_dir = super::super::create_native_batch_temp_dir()?;
    let compile_result = (|| -> io::Result<(crate::backend_process::input::ContentIdentity, Vec<super::super::NativeWorkerOrigin>)> {
        let batch_specs = jobs::materialize_native_application_batch_jobs(
            &tmp_dir,
            plan,
            options,
            &environment,
            batch_ops_budget,
        )?;
        let cache = options.object_cache.as_ref();
        let keys = plan_record.as_ref().map(|_| {
            batch_specs.iter().map(|spec| spec.input_key.expect("keyed application plan")).collect::<Vec<_>>()
        });
        let output = run::run_native_application_batches(output_path, batch_specs, options.log_prefix, cache)?;
        if let (Some(record), Some(keys), Some(cache)) = (plan_record.as_mut(), keys, cache)
            && cache.executable_unchanged()?
        {
            record.publish(&keys)?;
        }
        Ok(output)
    })();

    let (output, workers) = super::super::finish_native_batch_temp_dir(
        &tmp_dir,
        "native application batch temp dir",
        compile_result,
    )?;

    eprintln!(
        "Successfully compiled to {} ({} functions, {} batches)",
        output_path.display(),
        function_count,
        total_batches
    );
    Ok(NativeApplicationArtifactResult {
        function_count,
        batch_count: total_batches,
        codegen: crate::backend_process::origin::CodegenOrigin::NativeWorkers { workers },
        output,
    })
}
