use std::path::Path;

use super::super::files::sha256_file_hex;
use super::super::paths::stdlib_cache_object_digest_sidecar_path;
use super::read::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, read_stdlib_cache_partition_manifest,
};

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
