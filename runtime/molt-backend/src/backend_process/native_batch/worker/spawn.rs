use super::{admission::WorkerPermit, compile::NativeWorkerOrigin};
use std::{io, path::Path};

#[cfg(test)]
use super::compile::compile_native_batch_object_job_bytes;

/// Run one worker process on `job`, streamed to its stdin: no job file is
/// written for a worker that succeeds.
#[cfg(not(test))]
pub(super) fn run_native_batch_worker(
    job: &[u8],
    object_path: &Path,
    permit: WorkerPermit,
) -> io::Result<NativeWorkerOrigin> {
    use crate::backend_process::input::ContentIdentity;
    use std::io::{Read as _, Write as _};

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
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    if let Some(scope) = crate::backend_process::origin::current_scope() {
        command.arg("--emission-scope").arg(scope);
    }
    // A daemon worker gets its request's settings, not the daemon's.
    molt_backend::backend_environment::configure_child(&mut command);
    let mut child = command.spawn().map_err(|err| {
        io::Error::other(format!(
            "failed to spawn native batch worker '{}': {err}",
            exe.display()
        ))
    })?;
    // The worker reads the whole job before it compiles; its bounded receipt is
    // emitted only after consuming that complete input, so writing first cannot deadlock.
    let mut stdin = child.stdin.take().expect("worker stdin is piped");
    let streamed = stdin.write_all(job);
    drop(stdin);
    let process_id = child.id();
    let mut receipt = Vec::new();
    let read_result = child
        .stdout
        .take()
        .expect("worker stdout is piped")
        .take(16385)
        .read_to_end(&mut receipt);
    if read_result.is_err() || receipt.len() > 16384 {
        // This is the exact child handle this invocation spawned; always reap it.
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other(
            "native worker origin transport failed or exceeded its bound",
        ));
    }
    let (status, peak_bytes) = wait_with_resident_peak(&mut child).map_err(|err| {
        io::Error::other(format!("failed to wait for native batch worker: {err}"))
    })?;
    permit.finish(peak_bytes);
    if !status.success() {
        return Err(io::Error::other(format!(
            "native batch worker failed with {status}"
        )));
    }
    streamed.map_err(|err| {
        io::Error::other(format!(
            "failed to stream the job to its native batch worker: {err}"
        ))
    })?;
    let origin: NativeWorkerOrigin = serde_json::from_slice(&receipt)
        .map_err(|e| io::Error::other(format!("invalid native worker origin: {e}")))?;
    origin.validate()?;
    if origin.scope.as_deref() != crate::backend_process::origin::current_scope()
        || origin.process_id != process_id
        || origin.input != ContentIdentity::of_bytes(job)
    {
        return Err(io::Error::other(
            "native worker origin differs from spawned child or consumed job",
        ));
    }
    let mut output = molt_artifact_publish::open_regular_artifact(object_path)?;
    let mut digest = sha2::Sha256::default();
    let size = io::copy(&mut output, &mut DigestSink(&mut digest))?;
    use sha2::Digest as _;
    let identity = ContentIdentity {
        size,
        sha256: molt_ir::content_digest::bytes_to_lower_hex(&digest.finalize()),
    };
    if origin.output != identity {
        return Err(io::Error::other(
            "native worker origin names different output bytes",
        ));
    }
    Ok(origin)
}

/// Reap `child` and report its resident-set peak, the measurement worker
/// admission sizes later workers by.
#[cfg(all(not(test), unix))]
fn wait_with_resident_peak(
    child: &mut std::process::Child,
) -> io::Result<(std::process::ExitStatus, Option<u64>)> {
    use std::os::unix::process::ExitStatusExt as _;
    let pid = libc::pid_t::try_from(child.id())
        .map_err(|_| io::Error::other("worker process id exceeds pid_t"))?;
    let mut status: libc::c_int = 0;
    // SAFETY: an all-zero `rusage` is a valid value of the plain C struct.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `wait4` writes only into the two out-parameters it is
        // handed, for the child this invocation spawned and has not reaped.
        let reaped = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
        if reaped == pid {
            break;
        }
        let err = io::Error::last_os_error();
        if reaped == -1 && err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
    // `ru_maxrss` is bytes on Darwin and KiB on Linux and the BSDs.
    let max_rss = u64::try_from(usage.ru_maxrss).unwrap_or(0);
    let peak = if cfg!(target_vendor = "apple") {
        max_rss
    } else {
        max_rss.saturating_mul(1024)
    };
    Ok((std::process::ExitStatus::from_raw(status), Some(peak)))
}

/// Reap `child`; this platform reports no resident peak for it.
#[cfg(all(not(test), not(unix)))]
fn wait_with_resident_peak(
    child: &mut std::process::Child,
) -> io::Result<(std::process::ExitStatus, Option<u64>)> {
    Ok((child.wait()?, None))
}

#[cfg(test)]
pub(super) fn run_native_batch_worker(
    job: &[u8],
    object_path: &Path,
    permit: WorkerPermit,
) -> io::Result<NativeWorkerOrigin> {
    let origin = compile_native_batch_object_job_bytes(job, object_path);
    permit.finish(None);
    origin
}

#[cfg(not(test))]
struct DigestSink<'a>(&'a mut sha2::Sha256);
#[cfg(not(test))]
impl io::Write for DigestSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        use sha2::Digest as _;
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
