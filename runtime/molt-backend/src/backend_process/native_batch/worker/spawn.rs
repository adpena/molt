use std::{io, path::Path};

#[cfg(test)]
use super::compile::compile_native_batch_object_job_file;

#[cfg(not(test))]
pub(super) fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
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
pub(super) fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
    compile_native_batch_object_job_file(job_path, object_path)
}
