use std::{io, path::Path};

use crate::backend_process::atomic_publish::write_text_atomically;

use super::super::files::sha256_file_hex;
use super::super::paths::{
    stdlib_cache_count_sidecar_path, stdlib_cache_key_sidecar_path,
    stdlib_cache_manifest_sidecar_path, stdlib_cache_object_digest_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path,
};

pub(crate) fn write_shared_stdlib_cache_sidecars(
    stdlib_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    write_text_atomically(
        &stdlib_cache_count_sidecar_path(stdlib_path),
        &stdlib_count.to_string(),
    )?;
    write_optional_sidecar(&stdlib_cache_key_sidecar_path(stdlib_path), cache_key)?;
    write_optional_sidecar(
        &stdlib_cache_manifest_sidecar_path(stdlib_path),
        cache_manifest,
    )?;
    write_text_atomically(
        &stdlib_cache_partition_manifest_sidecar_path(stdlib_path),
        partition_manifest,
    )?;
    let object_digest = sha256_file_hex(stdlib_path)?;
    write_text_atomically(
        &stdlib_cache_object_digest_sidecar_path(stdlib_path),
        &object_digest,
    )?;
    Ok(())
}

fn write_optional_sidecar(path: &Path, value: Option<&str>) -> io::Result<()> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        return write_text_atomically(path, value);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
