use std::io;
use std::time::Instant;

use molt_backend::tir::pipeline_cache::request_parallel_map;
use molt_backend::{NativeBackendModuleContext, SimpleIR};

use super::super::super::native_batch::{
    MappedObject, NativeBatchObjectJob, NativeWorkerOrigin,
    append_referenced_external_declarations, batch_external_function_names,
    create_native_batch_temp_dir, finish_native_batch_temp_dir,
    release_native_backend_batch_memory_to_os, run_native_batch_worker_with_failure_artifacts,
};
use super::super::publish::StdlibObjectCache;
use super::plan::{StdlibBatchPlan, log_stdlib_batch, stdlib_batch_ops_budget};

/// One closed constituent job, its input key when reuse is active, and the
/// batch's function and operation counts for the progress log.
struct PlannedBatch {
    job: NativeBatchObjectJob,
    key: Option<[u8; 32]>,
    functions: usize,
    ops: usize,
}

/// The closed, hashed constituent jobs of one stdlib archive, in member order.
/// Planning compiles nothing; it fixes the archive's content identity.
pub(crate) struct PlannedStdlibArchive {
    batches: Vec<PlannedBatch>,
}

pub(super) fn plan_batched_stdlib_cache_archive(
    plan: StdlibBatchPlan,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    cache: Option<&StdlibObjectCache>,
) -> io::Result<PlannedStdlibArchive> {
    // Batches are independent. Each one closes its job and hashes it on the
    // request's pool; results keep batch order.
    let batches = request_parallel_map(plan.batches, |batch_funcs| {
        let functions = batch_funcs.len();
        let ops = batch_funcs.iter().map(|f| f.ops.len()).sum::<usize>();
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
            codegen_environment: molt_ir::backend_environment::NativeCodegenEnvironment::capture()?,
            target_triple: target_triple.map(str::to_owned),
            emit_app_callable_resolver: false,
            app_callable_manifest: None,
            external_function_names,
            module_registry: None,
        }
        .close_dependencies(&plan.module_context);
        let key = cache.map(|cache| cache.input_key(&job)).transpose()?;
        Ok::<_, io::Error>(PlannedBatch {
            job,
            key,
            functions,
            ops,
        })
    })
    .into_iter()
    .collect::<io::Result<Vec<_>>>()?;
    drop(plan.module_context);
    drop(plan.external_function_declarations);
    drop(plan.all_function_names);
    Ok(PlannedStdlibArchive { batches })
}

impl PlannedStdlibArchive {
    /// The content key of the archive these constituents assemble, when every
    /// constituent has an input key (reuse is active).
    pub(crate) fn archive_key(&self) -> Option<[u8; 32]> {
        let keys = self
            .batches
            .iter()
            .map(|batch| batch.key)
            .collect::<Option<Vec<_>>>()?;
        Some(StdlibObjectCache::archive_key(&keys))
    }

    /// Restore or compile every constituent and hand the ordered objects to
    /// `publish`, which assembles the archive.
    pub(crate) fn assemble(
        self,
        log_prefix: &str,
        cache: Option<&StdlibObjectCache>,
        publish: impl FnOnce(&[MappedObject], &[NativeWorkerOrigin]) -> io::Result<()>,
    ) -> io::Result<()> {
        if cache.is_none() {
            eprintln!(
                "{log_prefix}: stdlib object reuse inactive (no compiler manifest or explicit compilation diagnostics)"
            );
        }
        let stdlib_tmp_dir = create_native_batch_temp_dir()?;
        let compile_result = (|| -> io::Result<()> {
            let total = self.batches.len();
            let started = Instant::now();
            // Each batch maps its admitted cached object or streams its job to
            // a worker, on the request's pool. Results keep batch order; the
            // lowest-index failure is reported.
            let batches: Vec<(usize, PlannedBatch)> =
                self.batches.into_iter().enumerate().collect();
            let specs = request_parallel_map(batches, |(index, batch)| {
                log_stdlib_batch(
                    log_prefix,
                    index,
                    total,
                    batch.functions,
                    batch.ops,
                    stdlib_batch_ops_budget(),
                );
                if let (Some(cache), Some(input)) = (cache, &batch.key)
                    && let Some((object, origin)) = cache.admit(input)?
                {
                    return Ok(Constituent::Reused(object, origin));
                }
                // The job is self-contained and hashed above; the worker reads
                // it from stdin, and only a failure writes it to disk for replay.
                let object_path = stdlib_tmp_dir.join(format!("batch_{index}.o"));
                let job = batch.job.encode()?;
                eprintln!(
                    "{log_prefix}: compiling stdlib object miss {}/{}",
                    index + 1,
                    total
                );
                let origin = run_native_batch_worker_with_failure_artifacts(
                    "native stdlib batch worker",
                    &job,
                    &format!("batch_{index}.json"),
                    &object_path,
                )?;
                Ok::<_, io::Error>(Constituent::Emitted {
                    path: object_path,
                    key: batch.key,
                    origin,
                })
            })
            .into_iter()
            .collect::<io::Result<Vec<_>>>()?;
            release_native_backend_batch_memory_to_os();
            let hits = specs
                .iter()
                .filter(|spec| matches!(spec, Constituent::Reused(..)))
                .count();
            let reused_bytes = specs
                .iter()
                .map(|spec| match spec {
                    Constituent::Reused(object, _) => object.len(),
                    Constituent::Emitted { .. } => 0,
                })
                .sum::<u64>();
            let compile_elapsed = started.elapsed();
            let cache_started = Instant::now();
            // Each miss publishes under its own key's lock, so publication runs
            // on the pool too; the archive keeps batch order.
            let emitted_bytes = specs
                .iter()
                .map(|spec| match spec {
                    Constituent::Reused(..) => Ok(0),
                    Constituent::Emitted { path, .. } => Ok(std::fs::metadata(path)?.len()),
                })
                .sum::<io::Result<u64>>()?;
            let constituents = request_parallel_map(specs, |spec| match spec {
                Constituent::Reused(object, origin) => Ok((object, origin)),
                Constituent::Emitted { path, key, origin } => {
                    let origin = match (cache, key) {
                        (Some(cache), Some(key)) => cache.publish(&key, &path, &origin)?,
                        _ => origin,
                    };
                    Ok((MappedObject::file(&path)?, origin))
                }
            })
            .into_iter()
            .collect::<io::Result<Vec<_>>>()?;
            let (objects, origins): (Vec<_>, Vec<_>) = constituents.into_iter().unzip();
            let cache_elapsed = cache_started.elapsed();
            let publication_started = Instant::now();
            // Completed objects stay mapped until assembly and its short cache
            // lock finish. No archive-sized intermediate copy.
            publish(&objects, &origins)?;
            drop(objects);
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
}

enum Constituent {
    /// An admitted cache generation, mapped in place.
    Reused(MappedObject, NativeWorkerOrigin),
    /// A worker's private output, published under its input key when present.
    Emitted {
        path: std::path::PathBuf,
        key: Option<[u8; 32]>,
        origin: NativeWorkerOrigin,
    },
}
