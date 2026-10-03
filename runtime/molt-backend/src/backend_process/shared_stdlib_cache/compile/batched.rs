use std::io;
use std::path::Path;
use std::time::Instant;

use molt_backend::SimpleIR;

use super::super::super::io_limits::write_json_artifact;
use super::super::super::native_batch::{
    NativeBatchObjectJob, append_referenced_external_declarations, batch_external_function_names,
    finish_native_batch_temp_dir, release_native_backend_batch_memory_to_os,
    run_native_batch_worker_with_failure_artifacts, write_native_archive_objects,
};
use super::super::publish::StdlibObjectCache;
use super::plan::{StdlibBatchPlan, log_stdlib_batch, stdlib_batch_ops_budget};

pub(super) fn compile_batched_stdlib_cache_archive(
    stdlib_path: &Path,
    plan: StdlibBatchPlan,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
    cache: Option<&StdlibObjectCache>,
) -> io::Result<()> {
    let stdlib_tmp_dir = super::super::stdlib_cache_temp_publish_path(stdlib_path, "objects");
    std::fs::create_dir_all(&stdlib_tmp_dir)?;
    let compile_result = (|| -> io::Result<()> {
        let total = plan.batches.len();
        let mut specs = Vec::with_capacity(total);
        let mut reused_bytes = 0u64;
        let mut emitted_bytes = 0u64;
        let mut hits = 0usize;
        let started = Instant::now();
        for (index, batch_funcs) in plan.batches.into_iter().enumerate() {
            log_stdlib_batch(
                log_prefix,
                index,
                total,
                &batch_funcs,
                stdlib_batch_ops_budget(),
            );
            let mut ir = SimpleIR {
                functions: batch_funcs,
                profile: profile.clone(),
            };
            let external_function_names =
                batch_external_function_names(&plan.all_function_names, &ir.functions);
            append_referenced_external_declarations(
                &mut ir.functions,
                &plan.external_function_declarations,
            );
            let job = NativeBatchObjectJob {
                ir,
                module_context: plan.module_context.clone(),
                codegen_environment:
                    molt_ir::backend_environment::NativeCodegenEnvironment::capture()?,
                target_triple: target_triple.map(str::to_owned),
                emit_app_callable_resolver: false,
                app_callable_manifest: None,
                external_function_names,
                module_registry: None,
            }
            .close_dependencies();
            let job_path = stdlib_tmp_dir.join(format!("batch_{index}.json"));
            let object_path = stdlib_tmp_dir.join(format!("batch_{index}.o"));
            let key = cache.map(|cache| cache.input_key(&job)).transpose()?;
            // Preserve every closed input job for failure replay, including
            // hits. No worker can consult an un-hashed context side file.
            write_json_artifact(&job_path, &job)?;
            let reused = match (cache, &key) {
                (Some(cache), Some(key)) => cache.restore(key, &object_path)?,
                _ => None,
            };
            if let Some(bytes) = reused {
                hits += 1;
                reused_bytes += bytes;
            }
            specs.push((job_path, object_path, key, reused.is_some()));
        }
        // Release whole-program transport before entering the one worker lane.
        drop(plan.module_context);
        drop(plan.external_function_declarations);
        drop(plan.all_function_names);
        release_native_backend_batch_memory_to_os();
        let admission_elapsed = started.elapsed();
        let codegen_started = Instant::now();
        let mut paths = Vec::with_capacity(total);
        for (index, (job, object, key, hit)) in specs.into_iter().enumerate() {
            if !hit {
                eprintln!(
                    "{log_prefix}: compiling stdlib object miss {}/{}",
                    index + 1,
                    total
                );
                run_native_batch_worker_with_failure_artifacts(
                    "native stdlib batch worker",
                    &job,
                    &object,
                )?;
                emitted_bytes += match (cache, key) {
                    (Some(cache), Some(key)) => cache.publish(&key, &object)?,
                    _ => std::fs::metadata(&object)?.len(),
                };
                release_native_backend_batch_memory_to_os();
            }
            paths.push(object);
        }
        let codegen_elapsed = codegen_started.elapsed();
        let assembly_started = Instant::now();
        write_native_archive_objects(stdlib_path, &paths)?;
        eprintln!(
            "{log_prefix}: stdlib object reuse: {} hits, {} misses, {} reused bytes, {} emitted bytes; admission {:?}, codegen {:?}, assembly {:?}",
            hits,
            total - hits,
            reused_bytes,
            emitted_bytes,
            admission_elapsed,
            codegen_elapsed,
            assembly_started.elapsed(),
        );
        Ok(())
    })();
    finish_native_batch_temp_dir(
        &stdlib_tmp_dir,
        "native stdlib batch temp dir",
        compile_result,
    )
}
