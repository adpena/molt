use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn stdlib_cache_count_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("count")
}

pub(super) fn stdlib_cache_key_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("key")
}

pub(super) fn stdlib_cache_manifest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("manifest.json")
}

pub(crate) fn stdlib_cache_partition_manifest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("partition.json")
}

pub(super) fn stdlib_cache_archive_digest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("sha256")
}

/// Python derives these projections from this archive generation. They share
/// its publication lock and are invalidated on replacement or generation abort.
pub(super) fn stdlib_cache_derived_sidecar_paths(stdlib_path: &Path) -> [PathBuf; 2] {
    [
        stdlib_path.with_extension("symbol-contract.json"),
        stdlib_path.with_extension("symbols.json"),
    ]
}

pub(super) fn stdlib_cache_publish_lock_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_file_name(format!(
        "{}.publish.lock",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared")
    ))
}

pub(crate) fn stdlib_cache_temp_publish_path(stdlib_path: &Path, label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    stdlib_path.with_file_name(format!(
        ".{}.{}.{}.{}.tmp",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared"),
        std::process::id(),
        stamp,
        label,
    ))
}
