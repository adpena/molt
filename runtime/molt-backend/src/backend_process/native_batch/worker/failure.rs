use std::{io, path::Path};

use super::super::NativeBatchObjectJob;
use super::super::temp::preserve_native_batch_worker_failure_artifacts;
use super::admission::WorkerPermit;
use super::compile::NativeWorkerOrigin;
use super::spawn::run_native_batch_worker;

pub(crate) fn run_native_batch_worker_with_failure_artifacts(
    label: &str,
    job: NativeBatchObjectJob,
    job_name: &str,
    object_path: &Path,
) -> io::Result<NativeWorkerOrigin> {
    // Queued batches hold only their typed job until admission. The same
    // reservation covers serialization, transfer and the worker itself.
    let permit = WorkerPermit::acquire();
    let encoded = job.encode()?;
    drop(job);
    match run_native_batch_worker(&encoded, object_path, permit) {
        Ok(origin) => Ok(origin),
        Err(err) => {
            let original_error = format!("{label} job '{job_name}': {err}");
            match preserve_native_batch_worker_failure_artifacts(
                label,
                &encoded,
                job_name,
                object_path,
            ) {
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
