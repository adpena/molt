use std::{io, path::Path};

#[cfg(test)]
use super::compile::compile_native_batch_object_job_bytes;

/// Run one worker process on `job`, streamed to its stdin: no job file is
/// written for a worker that succeeds.
#[cfg(not(test))]
pub(super) fn run_native_batch_worker(job: &[u8], object_path: &Path) -> io::Result<()> {
    use std::io::Write as _;

    let exe = std::env::current_exe().map_err(|err| {
        io::Error::other(format!(
            "failed to resolve current backend executable for batch worker: {err}"
        ))
    })?;
    let mut command = std::process::Command::new(&exe);
    command
        .arg("--native-batch-job-file")
        .arg("-")
        .arg("--output")
        .arg(object_path)
        .stdin(std::process::Stdio::piped());
    // A daemon worker gets its request's settings, not the daemon's.
    molt_backend::backend_environment::configure_child(&mut command);
    let mut child = command.spawn().map_err(|err| {
        io::Error::other(format!(
            "failed to spawn native batch worker '{}': {err}",
            exe.display()
        ))
    })?;
    // The worker reads the whole job before it compiles, and its output is
    // not piped, so writing the job first cannot deadlock.
    let mut stdin = child.stdin.take().expect("worker stdin is piped");
    let streamed = stdin.write_all(job);
    drop(stdin);
    let status = child.wait().map_err(|err| {
        io::Error::other(format!("failed to wait for native batch worker: {err}"))
    })?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "native batch worker failed with {status}"
        )));
    }
    streamed.map_err(|err| {
        io::Error::other(format!(
            "failed to stream the job to its native batch worker: {err}"
        ))
    })
}

#[cfg(test)]
pub(super) fn run_native_batch_worker(job: &[u8], object_path: &Path) -> io::Result<()> {
    compile_native_batch_object_job_bytes(job, object_path)
}
