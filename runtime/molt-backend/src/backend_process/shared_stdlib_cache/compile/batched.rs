use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::super::io_limits::write_json_artifact;
use super::super::super::native_batch::{
    NativeBatchJobSpec, NativeBatchModuleMetadata, NativeBatchObjectJob,
    append_referenced_inherited_declarations, batch_external_function_names,
    finish_native_batch_temp_dir, merge_relocatable_objects,
    release_native_backend_batch_memory_to_os, run_native_batch_worker_with_failure_artifacts,
};
use super::plan::{StdlibBatchPlan, log_stdlib_batch, stdlib_batch_ops_budget};

pub(super) fn compile_batched_stdlib_cache_object(
    stdlib_path: &Path,
    plan: StdlibBatchPlan,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
) -> io::Result<()> {
    let stdlib_tmp_dir =
        std::env::temp_dir().join(format!("molt_stdlib_batch_{}", std::process::id()));
    std::fs::create_dir_all(&stdlib_tmp_dir)?;
    let module_context_path = stdlib_tmp_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata {
            module_context: plan.module_context,
        },
    )?;
    let mut stdlib_batch_specs: Vec<NativeBatchJobSpec> = Vec::new();
    let compile_result = (|| -> io::Result<()> {
        let stdlib_total_batches = plan.batches.len();
        let stdlib_batch_ops_budget = stdlib_batch_ops_budget();
        for (stdlib_batch_idx, batch_funcs) in plan.batches.into_iter().enumerate() {
            log_stdlib_batch(
                log_prefix,
                stdlib_batch_idx,
                stdlib_total_batches,
                &batch_funcs,
                stdlib_batch_ops_budget,
            );
            let mut batch_ir = SimpleIR {
                functions: batch_funcs,
                profile: profile.clone(),
            };
            let external_function_names =
                batch_external_function_names(&plan.all_function_names, &batch_ir.functions);
            append_referenced_inherited_declarations(
                &mut batch_ir.functions,
                &plan.inherited_function_declarations,
            );
            let job_path = stdlib_tmp_dir.join(format!("batch_{stdlib_batch_idx}.json"));
            let batch_path = stdlib_tmp_dir.join(format!("batch_{stdlib_batch_idx}.o"));
            write_json_artifact(
                &job_path,
                &NativeBatchObjectJob {
                    ir: batch_ir,
                    module_context_path: module_context_path.clone(),
                    target_triple: target_triple.map(str::to_owned),
                    emit_app_callable_resolver: false,
                    app_callable_manifest: None,
                    external_function_names,
                    // Stdlib cache objects never own the registry blob; the
                    // main application object emits it.
                    module_registry: None,
                },
            )?;
            stdlib_batch_specs.push(NativeBatchJobSpec {
                job_path,
                object_path: batch_path,
            });
        }
        release_native_backend_batch_memory_to_os();

        let mut stdlib_batch_paths: Vec<std::path::PathBuf> =
            Vec::with_capacity(stdlib_batch_specs.len());
        for (stdlib_batch_idx, spec) in stdlib_batch_specs.iter().enumerate() {
            eprintln!(
                "{log_prefix}: compiling materialized stdlib batch {}/{}",
                stdlib_batch_idx + 1,
                stdlib_total_batches
            );
            run_native_batch_worker_with_failure_artifacts(
                "native stdlib batch worker",
                &spec.job_path,
                &spec.object_path,
            )?;
            stdlib_batch_paths.push(spec.object_path.clone());
            release_native_backend_batch_memory_to_os();
        }

        merge_relocatable_objects(stdlib_path, &stdlib_batch_paths, None)
    })();

    finish_native_batch_temp_dir(
        &stdlib_tmp_dir,
        "native stdlib batch temp dir",
        compile_result,
    )
}
