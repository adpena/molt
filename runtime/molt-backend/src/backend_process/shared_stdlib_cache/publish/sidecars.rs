use std::io;
use std::path::Path;

use super::files::{sha256_file_hex, write_atomic_text_file};
use super::paths::{
    stdlib_cache_count_sidecar_path, stdlib_cache_key_sidecar_path,
    stdlib_cache_manifest_sidecar_path, stdlib_cache_object_digest_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path,
};

pub(crate) fn read_stdlib_cache_key(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_key_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn read_stdlib_cache_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn read_stdlib_cache_partition_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_partition_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn remove_shared_stdlib_cache_artifacts(stdlib_path: &Path) {
    let _ = std::fs::remove_file(stdlib_path);
    let _ = std::fs::remove_file(stdlib_cache_count_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_key_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_object_digest_sidecar_path(stdlib_path));
}

pub(crate) fn shared_stdlib_cache_matches(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> bool {
    let Some(expected_key) = expected_key.filter(|key| !key.is_empty()) else {
        return false;
    };
    let Some(expected_manifest) = expected_manifest.filter(|manifest| !manifest.is_empty()) else {
        return false;
    };
    if read_stdlib_cache_key(stdlib_path).as_deref() != Some(expected_key)
        || read_stdlib_cache_manifest(stdlib_path).as_deref() != Some(expected_manifest)
    {
        return false;
    };
    let Ok(actual_object_digest) = sha256_file_hex(stdlib_path) else {
        return false;
    };
    let Ok(cached_object_digest) =
        std::fs::read_to_string(stdlib_cache_object_digest_sidecar_path(stdlib_path))
    else {
        return false;
    };
    if cached_object_digest.trim() != actual_object_digest {
        return false;
    }
    let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path);
    if let Some(expected_partition_manifest) =
        expected_partition_manifest.filter(|manifest| !manifest.is_empty())
    {
        return cached_partition_manifest.as_deref() == Some(expected_partition_manifest);
    }
    cached_partition_manifest.is_some()
}

pub(crate) fn write_shared_stdlib_cache_sidecars(
    stdlib_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let count_path = stdlib_cache_count_sidecar_path(stdlib_path);
    write_atomic_text_file(&count_path, &stdlib_count.to_string())?;

    let key_path = stdlib_cache_key_sidecar_path(stdlib_path);
    if let Some(cache_key) = cache_key.filter(|key| !key.is_empty()) {
        write_atomic_text_file(&key_path, cache_key)?;
    } else {
        match std::fs::remove_file(&key_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    let manifest_path = stdlib_cache_manifest_sidecar_path(stdlib_path);
    if let Some(cache_manifest) = cache_manifest.filter(|manifest| !manifest.is_empty()) {
        write_atomic_text_file(&manifest_path, cache_manifest)?;
    } else {
        match std::fs::remove_file(&manifest_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    write_atomic_text_file(
        &stdlib_cache_partition_manifest_sidecar_path(stdlib_path),
        partition_manifest,
    )?;
    let object_digest = sha256_file_hex(stdlib_path)?;
    write_atomic_text_file(
        &stdlib_cache_object_digest_sidecar_path(stdlib_path),
        &object_digest,
    )?;
    Ok(())
}
