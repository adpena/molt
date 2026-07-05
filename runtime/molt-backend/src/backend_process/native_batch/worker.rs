use std::io;
use std::path::Path;

use molt_backend::SimpleBackend;

use super::super::io_limits::{read_json_artifact, write_output_path};
use super::batching::{NativeBatchModuleMetadata, NativeBatchObjectJob};
use super::temp::preserve_native_batch_worker_failure_artifacts;

pub(crate) fn compile_native_batch_object_job(
    job: NativeBatchObjectJob,
    output_path: &Path,
) -> io::Result<()> {
    let metadata: NativeBatchModuleMetadata =
        read_json_artifact(&job.module_context_path, "native batch module metadata")?;
    let mut backend = SimpleBackend::new_with_target(job.target_triple.as_deref());
    backend.skip_ir_passes = true;
    backend.skip_shared_stdlib_partition = true;
    backend.emit_app_callable_resolver = job.emit_app_callable_resolver;
    backend.app_callable_manifest = job.app_callable_manifest;
    backend.external_function_names = job.external_function_names;
    backend.module_registry = job.module_registry;
    backend.set_module_context(metadata.module_context);
    let output = backend.compile(job.ir);
    write_output_path(output_path, &output.bytes)
}

pub(crate) fn compile_native_batch_object_job_file(
    job_path: &Path,
    output_path: &Path,
) -> io::Result<()> {
    let job: NativeBatchObjectJob = read_json_artifact(job_path, "native batch object job")?;
    compile_native_batch_object_job(job, output_path)
}

pub(crate) fn run_native_batch_worker_with_failure_artifacts(
    label: &str,
    job_path: &Path,
    object_path: &Path,
) -> io::Result<()> {
    match run_native_batch_worker(job_path, object_path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let original_error = err.to_string();
            match preserve_native_batch_worker_failure_artifacts(label, job_path, object_path) {
                Ok(artifact_dir) => Err(io::Error::new(
                    err.kind(),
                    format!(
                        "{original_error}; preserved replayable {label} artifacts at '{}'",
                        artifact_dir.display()
                    ),
                )),
                Err(preserve_err) => Err(io::Error::new(
                    err.kind(),
                    format!(
                        "{original_error}; additionally failed to preserve {label} artifacts: {preserve_err}"
                    ),
                )),
            }
        }
    }
}

#[cfg(not(test))]
pub(crate) fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
    let exe = std::env::current_exe().map_err(|err| {
        io::Error::other(format!(
            "failed to resolve current backend executable for batch worker: {err}"
        ))
    })?;
    let status = std::process::Command::new(&exe)
        .arg("--native-batch-job-file")
        .arg(job_path)
        .arg("--output")
        .arg(object_path)
        .status()
        .map_err(|err| {
            io::Error::other(format!(
                "failed to spawn native batch worker '{}' for '{}': {err}",
                exe.display(),
                job_path.display()
            ))
        })?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "native batch worker failed for '{}' with {status}",
        job_path.display()
    )))
}

#[cfg(test)]
pub(crate) fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
    compile_native_batch_object_job_file(job_path, object_path)
}
