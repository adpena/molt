use std::io;
use std::path::PathBuf;
use std::time::Instant;

use molt_backend::{NativeBackendModuleContext, SimpleIR};
use molt_backend::tir::pipeline_cache::request_parallel_map;

use super::super::super::io_limits::write_json_artifact;
use super::super::super::native_batch::{
    NativeBatchObjectJob, NativeBatchWork, append_referenced_external_declarations,
    batch_external_function_names, create_native_batch_temp_dir, finish_native_batch_temp_dir,
    release_native_backend_batch_memory_to_os, run_native_batch_workers,
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
        // Batches are independent: admit them (close, hash, persist, restore)
        // on the request's pool. Specs keep batch order; the lowest-index
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
            Ok::<_, io::Error>((job_path, object_path, key, reused))
        })
        .into_iter()
        .collect::<io::Result<Vec<_>>>()?;
        for (_, _, _, reused) in &specs {
            if let Some(bytes) = reused {
                hits += 1;
                reused_bytes += bytes;
            }
        }
        // Release whole-program transport before entering the one worker lane.
        drop(plan.module_context);
        drop(plan.external_function_declarations);
        drop(plan.all_function_names);
        release_native_backend_batch_memory_to_os();
        let admission_elapsed = started.elapsed();
        let codegen_started = Instant::now();
        // Cache misses compile in parallel worker processes on the request's
        // pool; hits and publication keep batch order.
        let works: Vec<NativeBatchWork<'_>> = specs
            .iter()
            .enumerate()
            .filter(|(_, (_, _, _, reused))| reused.is_none())
            .map(|(index, (job, object, _, _))| NativeBatchWork {
                job_path: job,
                object_path: object,
                progress: format!(
                    "{log_prefix}: compiling stdlib object miss {}/{}",
                    index + 1,
                    total
                ),
            })
            .collect();
        run_native_batch_workers("native stdlib batch worker", &works)?;
        release_native_backend_batch_memory_to_os();
        // Each miss publishes under its own key's lock, so publication runs
        // on the pool too; the archive keeps batch order.
        emitted_bytes += request_parallel_map(&specs, |(_, object, key, reused)| {
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
        let paths: Vec<PathBuf> = specs.into_iter().map(|(_, object, _, _)| object).collect();
        let codegen_elapsed = codegen_started.elapsed();
        let publication_started = Instant::now();
        // Completed objects remain owned until final assembly/publication and
        // its short cache lock finish. No archive-sized intermediate copy.
        publish(&paths)?;
        eprintln!(
            "{log_prefix}: stdlib object reuse: {} hits, {} misses, {} reused bytes, {} emitted bytes; admission {:?}, codegen {:?}, publication {:?}",
            hits,
            total - hits,
            reused_bytes,
            emitted_bytes,
            admission_elapsed,
            codegen_elapsed,
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
