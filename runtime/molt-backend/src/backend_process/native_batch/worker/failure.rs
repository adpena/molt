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

/// One batch worker to run: its job file, its output object and the progress
/// line it prints when it starts.
pub(crate) struct NativeBatchWork<'a> {
    pub(crate) job_path: &'a Path,
    pub(crate) object_path: &'a Path,
    pub(crate) progress: String,
}

/// Run `works` concurrently on the request's compiler pool. Each worker is its
/// own process; the pool's width (the request's CPU limit, shared with
/// concurrent compiles) bounds how many run at once. Returns the failure of the
/// lowest-index work, so the reported error does not depend on scheduling.
pub(crate) fn run_native_batch_workers(
    label: &str,
    works: &[NativeBatchWork<'_>],
) -> io::Result<()> {
    molt_backend::tir::pipeline_cache::request_parallel_map(works, |work| {
        eprintln!("{}", work.progress);
        run_native_batch_worker_with_failure_artifacts(label, work.job_path, work.object_path)
    })
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::{NativeBatchWork, run_native_batch_workers};
    use std::path::PathBuf;

    #[test]
    fn the_lowest_index_failure_is_reported_whatever_the_schedule() {
        let dir = std::env::temp_dir().join(format!("molt-batch-workers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<(PathBuf, PathBuf)> = (0..8)
            .map(|index| {
                (
                    dir.join(format!("missing-job-{index}.json")),
                    dir.join(format!("object-{index}.o")),
                )
            })
            .collect();
        let works: Vec<NativeBatchWork<'_>> = paths
            .iter()
            .enumerate()
            .map(|(index, (job, object))| NativeBatchWork {
                job_path: job,
                object_path: object,
                progress: format!("test batch {index}"),
            })
            .collect();
        for _ in 0..4 {
            let error = run_native_batch_workers("test batch worker", &works)
                .expect_err("every job file is missing");
            assert!(
                error.to_string().contains("missing-job-0.json"),
                "reported {error}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
