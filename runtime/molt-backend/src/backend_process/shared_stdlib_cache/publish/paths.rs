use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn stdlib_cache_count_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("count")
}

pub(crate) fn stdlib_cache_key_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("key")
}

pub(crate) fn stdlib_cache_manifest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("manifest.json")
}

pub(crate) fn stdlib_cache_partition_manifest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("partition.json")
}

pub(crate) fn stdlib_cache_object_digest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("sha256")
}

pub(crate) fn stdlib_cache_publish_lock_path(stdlib_path: &Path) -> PathBuf {
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
