use std::path::Path;

use super::super::files::sha256_file_hex;
use super::super::paths::stdlib_cache_archive_digest_sidecar_path;
use super::read::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, read_stdlib_cache_partition_manifest,
};

/// The recorded archive digest of a generation whose identity sidecars match
/// the expected key, manifest and partition. Reads only the small sidecars;
/// the payload still needs [`shared_stdlib_archive_payload_matches`].
pub(in super::super) fn shared_stdlib_cache_recorded_digest(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> Option<String> {
    let expected_key = expected_key.filter(|key| !key.is_empty())?;
    let expected_manifest = expected_manifest.filter(|manifest| !manifest.is_empty())?;
    if read_stdlib_cache_key(stdlib_path).as_deref() != Some(expected_key)
        || read_stdlib_cache_manifest(stdlib_path).as_deref() != Some(expected_manifest)
    {
        return None;
    }
    let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path)?;
    if let Some(expected_partition_manifest) =
        expected_partition_manifest.filter(|manifest| !manifest.is_empty())
        && cached_partition_manifest != expected_partition_manifest
    {
        return None;
    }
    let recorded = std::fs::read_to_string(stdlib_cache_archive_digest_sidecar_path(stdlib_path))
        .ok()?
        .trim()
        .to_string();
    (!recorded.is_empty()).then_some(recorded)
}

/// Whether the archive bytes at `path` have `digest` and pass the producer's
/// archive admission. This reads the whole payload.
pub(in super::super) fn shared_stdlib_archive_payload_matches(path: &Path, digest: &str) -> bool {
    let Ok(actual_archive_digest) = sha256_file_hex(path) else {
        return false;
    };
    if actual_archive_digest != digest {
        return false;
    }
    if let Err(error) = crate::backend_process::native_batch::validate_native_archive_file(path) {
        eprintln!(
            "MOLT_BACKEND: shared stdlib archive admission failed for '{}': {error}",
            path.display()
        );
        return false;
    }
    true
}

pub(in super::super) fn shared_stdlib_cache_matches_unlocked(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> bool {
    shared_stdlib_cache_recorded_digest(
        stdlib_path,
        expected_key,
        expected_manifest,
        expected_partition_manifest,
    )
    .is_some_and(|digest| shared_stdlib_archive_payload_matches(stdlib_path, &digest))
}
