mod artifacts;
mod cleanup;
mod sanitize;

pub(crate) use artifacts::preserve_native_batch_worker_failure_artifacts;
pub(crate) use cleanup::finish_native_batch_temp_dir;
#[cfg(test)]
pub(crate) use cleanup::remove_native_batch_temp_dir;

/// Reserve one native compilation workspace outside evictable cache entries.
/// Application and stdlib batches use the same scratch and cleanup authority.
#[expect(
    clippy::disallowed_methods,
    reason = "the guard scratch root belongs to the process, never to a request"
)]
pub(crate) fn create_native_batch_temp_dir() -> std::io::Result<std::path::PathBuf> {
    // One-shot and non-exported daemons inherit their guard's allocation. A
    // suite-owned daemon removes that variable and binds its platform temp
    // variables to the suite lease's scratch at spawn. Never take requester's
    // scratch through the per-request codegen environment.
    let root = match std::env::var_os("MOLT_GUARD_SCRATCH_ROOT") {
        Some(root) => {
            let root = std::path::PathBuf::from(root);
            if !root.is_absolute() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "native batch guard scratch must be absolute",
                ));
            }
            if !std::fs::metadata(&root)?.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    "native batch guard scratch is not a directory",
                ));
            }
            root
        }
        None => std::env::temp_dir(),
    };
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = root.join(format!(
        "molt_batch_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&path)?;
    Ok(path)
}
