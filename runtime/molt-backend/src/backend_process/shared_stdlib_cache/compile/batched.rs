use std::io;
use std::path::PathBuf;
use std::time::Instant;

use molt_backend::tir::pipeline_cache::request_parallel_map;
use molt_backend::{NativeBackendModuleContext, SimpleIR};

use super::super::super::native_batch::{
    NativeBatchObjectJob, append_referenced_external_declarations, batch_external_function_names,
    create_native_batch_temp_dir, finish_native_batch_temp_dir,
    release_native_backend_batch_memory_to_os, run_native_batch_worker_with_failure_artifacts,
};
use super::super::publish::StdlibObjectCache;
use super::plan::{StdlibBatchPlan, log_stdlib_batch, stdlib_batch_ops_budget};

pub(super) fn compile_batched_stdlib_cache_archive(
    plan: StdlibBatchPlan,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
    cache: Option<&StdlibObjectCache>,
    publish: impl FnOnce(&[PathBuf]) -> io::Result<()>,
) -> io::Result<()> {
    let stdlib_tmp_dir = create_native_batch_temp_dir()?;
    let compile_result = (|| -> io::Result<()> {
        let total = plan.batches.len();
        let mut reused_bytes = 0u64;
        let mut emitted_bytes = 0u64;
        let mut hits = 0usize;
        let started = Instant::now();
        // Batches are independent. Each one closes its job, hashes it, and
        // either restores its cached object or streams the job to a worker,
        // all on the request's pool. Results keep batch order; the lowest-index
        // failure is reported.
        let batches: Vec<(usize, Vec<molt_backend::FunctionIR>)> =
            plan.batches.into_iter().enumerate().collect();
        let specs = request_parallel_map(batches, |(index, batch_funcs)| {
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
                code_slots: None,
                module_context: NativeBackendModuleContext::default(),
                codegen_environment:
                    molt_ir::backend_environment::NativeCodegenEnvironment::capture()?,
                target_triple: target_triple.map(str::to_owned),
                emit_app_callable_resolver: false,
                app_callable_manifest: None,
                external_function_names,
                module_registry: None,
            }
            .close_dependencies(&plan.module_context);
            let object_path = stdlib_tmp_dir.join(format!("batch_{index}.o"));
            let key = cache.map(|cache| cache.input_key(&job)).transpose()?;
            if let (Some(cache), Some(input)) = (cache, &key)
                && let Some(bytes) = cache.restore(input, &object_path)?
            {
                return Ok((object_path, key, Some(bytes)));
            }
            // The job is self-contained and hashed above; the worker reads it
            // from stdin, and only a failure writes it to disk for replay.
            let job = job.encode()?;
            eprintln!(
                "{log_prefix}: compiling stdlib object miss {}/{}",
                index + 1,
                total
            );
            run_native_batch_worker_with_failure_artifacts(
                "native stdlib batch worker",
                &job,
                &format!("batch_{index}.json"),
                &object_path,
            )?;
            Ok::<_, io::Error>((object_path, key, None))
        })
        .into_iter()
        .collect::<io::Result<Vec<_>>>()?;
        for (_, _, reused) in &specs {
            if let Some(bytes) = reused {
                hits += 1;
                reused_bytes += bytes;
            }
        }
        drop(plan.module_context);
        drop(plan.external_function_declarations);
        drop(plan.all_function_names);
        release_native_backend_batch_memory_to_os();
        let compile_elapsed = started.elapsed();
        let cache_started = Instant::now();
        // Each miss publishes under its own key's lock, so publication runs
        // on the pool too; the archive keeps batch order.
        emitted_bytes += request_parallel_map(&specs, |(object, key, reused)| {
            if reused.is_some() {
                return Ok(0);
            }
            match (cache, key) {
                (Some(cache), Some(key)) => cache.publish(key, object),
                _ => Ok(std::fs::metadata(object)?.len()),
            }
        })
        .into_iter()
        .sum::<io::Result<u64>>()?;
        let paths: Vec<PathBuf> = specs.into_iter().map(|(object, _, _)| object).collect();
        let cache_elapsed = cache_started.elapsed();
        let publication_started = Instant::now();
        // Completed objects remain owned until final assembly/publication and
        // its short cache lock finish. No archive-sized intermediate copy.
        publish(&paths)?;
        eprintln!(
            "{log_prefix}: stdlib object reuse: {} hits, {} misses, {} reused bytes, {} emitted bytes; compile {:?}, cache {:?}, publication {:?}",
            hits,
            total - hits,
            reused_bytes,
            emitted_bytes,
            compile_elapsed,
            cache_elapsed,
            publication_started.elapsed(),
        );
        Ok(())
    })();
    finish_native_batch_temp_dir(
        &stdlib_tmp_dir,
        "native stdlib batch temp dir",
        compile_result,
    )
}
