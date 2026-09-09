use std::{
    io,
    path::{Path, PathBuf},
};

use super::super::paths::{
    stdlib_cache_archive_digest_sidecar_path, stdlib_cache_count_sidecar_path,
    stdlib_cache_derived_sidecar_paths, stdlib_cache_key_sidecar_path,
    stdlib_cache_manifest_sidecar_path, stdlib_cache_partition_manifest_sidecar_path,
};

/// Only a publisher or a locked admission decision owns invalidation.
pub(in super::super) fn remove_shared_stdlib_cache_artifacts(stdlib_path: &Path) -> io::Result<()> {
    let paths = [
        stdlib_path.to_path_buf(),
        stdlib_cache_count_sidecar_path(stdlib_path),
        stdlib_cache_key_sidecar_path(stdlib_path),
        stdlib_cache_manifest_sidecar_path(stdlib_path),
        stdlib_cache_partition_manifest_sidecar_path(stdlib_path),
        stdlib_cache_archive_digest_sidecar_path(stdlib_path),
    ];
    remove_generation_paths(
        paths
            .into_iter()
            .chain(stdlib_cache_derived_sidecar_paths(stdlib_path)),
    )
}

pub(super) fn remove_shared_stdlib_derived_sidecars(stdlib_path: &Path) -> io::Result<()> {
    remove_generation_paths(stdlib_cache_derived_sidecar_paths(stdlib_path))
}

fn remove_generation_paths(paths: impl IntoIterator<Item = PathBuf>) -> io::Result<()> {
    let mut failures = Vec::new();
    for path in paths {
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != io::ErrorKind::NotFound
        {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "shared stdlib invalidation failed: {}",
            failures.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::write::write_shared_stdlib_cache_sidecars;
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn replacement_and_invalidation_retire_all_generation_projections() {
        let directory = std::env::temp_dir().join(format!(
            "molt-stdlib-projection-custody-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create fixture");
        let archive = directory.join("stdlib.a");
        std::fs::write(&archive, b"archive").expect("archive fixture");
        for invalidate in [false, true] {
            for path in stdlib_cache_derived_sidecar_paths(&archive) {
                std::fs::write(path, "old projection").expect("seed derived projection");
            }
            if invalidate {
                remove_shared_stdlib_cache_artifacts(&archive).expect("invalidate generation");
                assert!(!archive.exists());
            } else {
                write_shared_stdlib_cache_sidecars(
                    &archive,
                    0,
                    Some("key"),
                    Some("manifest"),
                    "partition",
                )
                .expect("publish replacement metadata");
                assert!(archive.exists());
            }
            for path in stdlib_cache_derived_sidecar_paths(&archive) {
                assert!(
                    !path.exists(),
                    "stale derived projection must not outlive generation"
                );
            }
        }
        // All authoritative and derived files are removed, not merely the archive.
        assert_eq!(
            std::fs::read_dir(&directory).expect("list fixture").count(),
            0
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn projection_cleanup_failure_is_loud_and_does_not_skip_siblings() {
        let directory = std::env::temp_dir().join(format!(
            "molt-stdlib-projection-failure-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create fixture");
        let archive = directory.join("stdlib.a");
        let [blocked, removable] = stdlib_cache_derived_sidecar_paths(&archive);
        std::fs::create_dir(&blocked).expect("block derived cleanup");
        std::fs::write(&removable, "old projection").expect("seed removable sibling");
        let error = remove_shared_stdlib_derived_sidecars(&archive)
            .expect_err("projection cleanup failure propagates");
        assert!(error.to_string().contains("symbol-contract.json"));
        assert!(blocked.exists());
        assert!(
            !removable.exists(),
            "one failure cannot skip remaining owned paths"
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }
}
