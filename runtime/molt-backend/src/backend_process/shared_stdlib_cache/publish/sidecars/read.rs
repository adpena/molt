use std::path::Path;

use super::super::paths::{
    stdlib_cache_key_sidecar_path, stdlib_cache_manifest_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path,
};

pub(crate) fn read_stdlib_cache_key(stdlib_path: &Path) -> Option<String> {
    read_trimmed_sidecar(&stdlib_cache_key_sidecar_path(stdlib_path))
}

pub(crate) fn read_stdlib_cache_manifest(stdlib_path: &Path) -> Option<String> {
    read_trimmed_sidecar(&stdlib_cache_manifest_sidecar_path(stdlib_path))
}

pub(crate) fn read_stdlib_cache_partition_manifest(stdlib_path: &Path) -> Option<String> {
    read_trimmed_sidecar(&stdlib_cache_partition_manifest_sidecar_path(stdlib_path))
}

fn read_trimmed_sidecar(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
