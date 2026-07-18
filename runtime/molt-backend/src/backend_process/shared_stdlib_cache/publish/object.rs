use std::io;
use std::path::Path;

use super::lock::with_shared_stdlib_cache_publish_lock;
use super::sidecars::{remove_shared_stdlib_cache_artifacts, write_shared_stdlib_cache_sidecars};
use crate::backend_process::atomic_publish::commit_existing_file_atomically;

pub(crate) fn publish_shared_stdlib_cache_object(
    stdlib_path: &Path,
    temp_object_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let result = with_shared_stdlib_cache_publish_lock(stdlib_path, || {
        if let Err(err) = commit_existing_file_atomically(temp_object_path, stdlib_path) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        if let Err(err) = write_shared_stdlib_cache_sidecars(
            stdlib_path,
            stdlib_count,
            cache_key,
            cache_manifest,
            partition_manifest,
        ) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(temp_object_path);
    }
    result
}
