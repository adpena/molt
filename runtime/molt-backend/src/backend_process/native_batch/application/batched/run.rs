use std::io;
use std::path::Path;

use super::super::super::{
    MappedObject, NativeBatchJobSpec, release_native_backend_batch_memory_to_os,
    run_native_batch_worker_with_failure_artifacts, write_native_archive_objects,
};

pub(super) fn run_native_application_batches(
    output_path: &Path,
    batch_specs: Vec<NativeBatchJobSpec>,
    log_prefix: &str,
    cache: Option<&crate::backend_process::shared_stdlib_cache::NativeObjectCache>,
) -> io::Result<(
    crate::backend_process::input::ContentIdentity,
    Vec<super::super::super::NativeWorkerOrigin>,
)> {
    release_native_backend_batch_memory_to_os();

    // Batches compile in parallel worker processes on the request's pool; the
    // archive keeps batch order.
    let total = batch_specs.len();
    let jobs: Vec<_> = batch_specs.into_iter().enumerate().collect();
    let constituents =
        molt_backend::tir::pipeline_cache::request_parallel_map(jobs, |(batch_idx, spec)| {
            if let (Some(cache), Some(key)) = (cache, spec.input_key.as_ref())
                && let Some((object, origin)) = cache.admit(key)?
            {
                return Ok((object, origin, true));
            }
            eprintln!(
                "{log_prefix}: compiling materialized batch {}/{total}",
                batch_idx + 1
            );
            let origin = run_native_batch_worker_with_failure_artifacts(
                "native application batch worker",
                spec.job,
                &spec.job_name,
                &spec.object_path,
            )?;
            let origin = if let (Some(cache), Some(key)) = (cache, spec.input_key.as_ref()) {
                cache.publish(key, &spec.object_path, &origin)?
            } else {
                origin
            };
            Ok::<_, io::Error>((MappedObject::file(&spec.object_path)?, origin, false))
        })
        .into_iter()
        .collect::<io::Result<Vec<_>>>()?;
    release_native_backend_batch_memory_to_os();

    let hits = constituents.iter().filter(|(_, _, hit)| *hit).count();
    let (objects, origins): (Vec<_>, Vec<_>) = constituents
        .into_iter()
        .map(|(object, origin, _)| (object, origin))
        .unzip();
    let output = write_native_archive_objects(output_path, &objects)?;
    eprintln!(
        "{log_prefix}: application object reuse: {hits} hits, {} misses",
        total - hits
    );
    Ok((output, origins))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lowest_index_failure_is_reported_whatever_the_schedule() {
        let dir = std::env::temp_dir().join(format!("molt-batch-workers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let specs = || {
            (0..8)
                .map(|index| NativeBatchJobSpec {
                    input_key: None,
                    job_name: format!("invalid-job-{index}.msgpack"),
                    job: crate::NativeBatchObjectJob {
                        ir: molt_backend::SimpleIR {
                            functions: Vec::new(),
                            profile: None,
                        }
                        .into(),
                        // A reusable object cannot own the application's slots.
                        code_slots: Some(Default::default()),
                        module_context: Default::default(),
                        codegen_environment:
                            molt_ir::backend_environment::NativeCodegenEnvironment::capture()
                                .unwrap(),
                        target_triple: None,
                        emit_app_callable_resolver: false,
                        app_callable_manifest: None,
                        external_function_names: Default::default(),
                        module_registry: None,
                    },
                    object_path: dir.join(format!("object-{index}.o")),
                })
                .collect()
        };
        for _ in 0..4 {
            let error = run_native_application_batches(&dir.join("out.a"), specs(), "test", None)
                .expect_err("every job is invalid");
            assert!(
                error.to_string().contains("invalid-job-0.msgpack"),
                "reported {error}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
