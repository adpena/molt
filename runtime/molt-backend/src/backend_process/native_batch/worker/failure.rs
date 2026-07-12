use std::{io, path::Path};

use super::super::temp::preserve_native_batch_worker_failure_artifacts;
use super::spawn::run_native_batch_worker;

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
