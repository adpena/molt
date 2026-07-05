use std::path::Path;

use super::super::paths::{
    stdlib_cache_count_sidecar_path, stdlib_cache_key_sidecar_path,
    stdlib_cache_manifest_sidecar_path, stdlib_cache_object_digest_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path,
};

pub(crate) fn remove_shared_stdlib_cache_artifacts(stdlib_path: &Path) {
    let _ = std::fs::remove_file(stdlib_path);
    let _ = std::fs::remove_file(stdlib_cache_count_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_key_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_object_digest_sidecar_path(stdlib_path));
}
