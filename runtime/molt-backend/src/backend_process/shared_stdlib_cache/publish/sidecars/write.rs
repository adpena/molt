use std::{io, path::Path};

use super::cleanup::remove_shared_stdlib_derived_sidecars;
use molt_artifact_publish::write_text_atomically;

use super::super::files::sha256_file_hex;
use super::super::paths::{
    stdlib_cache_archive_digest_sidecar_path, stdlib_cache_count_sidecar_path,
    stdlib_cache_key_sidecar_path, stdlib_cache_manifest_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path,
};

/// Write the sidecars of the archive generation at `stdlib_path` and return
/// its digest. `archive_digest` is the digest of bytes this publication copied
/// from a verified generation; without it the archive file is hashed.
pub(crate) fn write_shared_stdlib_cache_sidecars(
    stdlib_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
    archive_digest: Option<&str>,
) -> io::Result<String> {
    // No projection of the replaced generation retains custody. Regeneration
    // is lazy; deletion failures abort publication through the owning caller.
    remove_shared_stdlib_derived_sidecars(stdlib_path)?;
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
    let archive_digest = match archive_digest {
        Some(digest) => digest.to_string(),
        None => sha256_file_hex(stdlib_path)?,
    };
    write_text_atomically(
        &stdlib_cache_archive_digest_sidecar_path(stdlib_path),
        &archive_digest,
    )?;
    Ok(archive_digest)
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
