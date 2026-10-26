use crate::backend_process::native_batch::NativeWorkerOrigin;
use std::io;
use std::path::Path;

use super::lock::with_shared_stdlib_cache_publish_lock;
use super::sidecars::{remove_shared_stdlib_cache_artifacts, write_shared_stdlib_cache_sidecars};
use molt_artifact_publish::{AtomicPublicationError, PublicationState};

/// Publish one archive generation and its sidecars under the entry's lock, and
/// return the archive digest the sidecars record. `publish_archive` replaces the
/// archive file and returns its digest when it copied verified bytes, or `None`
/// for newly written bytes, which are then hashed.
pub(crate) fn publish_shared_stdlib_cache_archive(
    stdlib_path: &Path,
    publish_archive: impl FnOnce(&Path) -> io::Result<Option<String>>,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
    origins: Option<&[NativeWorkerOrigin]>,
) -> io::Result<String> {
    with_shared_stdlib_cache_publish_lock(stdlib_path, || {
        // A pre-transfer failure preserves the prior generation. A replacement
        // followed by a sidecar failure does not: invalidate its now-mixed
        // sidecars while still holding the publication lock.
        let known_digest = match publish_archive(stdlib_path) {
            Ok(digest) => digest,
            Err(error) => {
                // The shared archive writer keeps the typed replacement state in
                // its io::Error. Preserve it while invalidating only a committed
                // mixed generation; ordinary pre-publication I/O leaves it intact.
                if error
                    .get_ref()
                    .is_some_and(|inner| inner.is::<AtomicPublicationError>())
                {
                    let publication = error
                        .into_inner()
                        .expect("typed publication error")
                        .downcast::<AtomicPublicationError>()
                        .expect("checked publication type");
                    return Err(abort_archive_publication(stdlib_path, *publication));
                }
                return Err(error);
            }
        };
        write_shared_stdlib_cache_sidecars(
            stdlib_path,
            stdlib_count,
            cache_key,
            cache_manifest,
            partition_manifest,
            known_digest.as_deref(),
            origins,
        )
        .map_err(|err| {
            // Replacement has committed. This is a fail-closed transaction
            // abort, not rollback: the old generation has already been replaced.
            abort_archive_publication(
                stdlib_path,
                AtomicPublicationError::new(stdlib_path, PublicationState::Replaced, err),
            )
        })
    })
}

fn abort_archive_publication(path: &Path, mut error: AtomicPublicationError) -> io::Error {
    if error.state() == PublicationState::Replaced
        && let Err(cleanup_error) = remove_shared_stdlib_cache_artifacts(path)
    {
        error.record_cleanup_error(cleanup_error);
    }
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_archive_replacement_preserves_previous_cache_custody() {
        let directory = std::env::temp_dir().join(format!(
            "molt-stdlib-publish-failure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create test directory");
        let destination = directory.join("stdlib.a");
        let previous = b"previous payload custody";
        std::fs::write(&destination, previous).expect("seed previous artifact");
        write_shared_stdlib_cache_sidecars(
            &destination,
            1,
            Some("old-key"),
            Some("old-manifest"),
            "old-partition",
            None,
            None,
        )
        .expect("seed previous sidecars");
        let missing = directory.join("missing-producer-output.a");
        assert!(
            publish_shared_stdlib_cache_archive(
                &destination,
                |destination| molt_artifact_publish::commit_existing_file_atomically(
                    &missing,
                    destination
                )
                .map(|()| None)
                .map_err(Into::into),
                2,
                Some("new-key"),
                Some("new-manifest"),
                "new-partition",
                None,
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(&destination).expect("previous artifact retained"),
            previous
        );
        assert_eq!(
            std::fs::read_to_string(destination.with_extension("key"))
                .expect("previous key retained"),
            "old-key"
        );
        assert_eq!(
            std::fs::read_to_string(destination.with_extension("manifest.json"))
                .expect("previous manifest retained"),
            "old-manifest"
        );
        std::fs::remove_dir_all(directory).expect("clean test directory");
    }

    #[test]
    fn post_replacement_failure_invalidates_mixed_generation_under_custody() {
        let directory = std::env::temp_dir().join(format!(
            "molt-stdlib-post-replace-failure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create fixture");
        let destination = directory.join("stdlib.a");
        std::fs::write(&destination, b"old").expect("seed old archive");
        write_shared_stdlib_cache_sidecars(
            &destination,
            1,
            Some("old-key"),
            Some("old-manifest"),
            "old-partition",
            None,
            None,
        )
        .expect("seed old sidecars");
        let error = publish_shared_stdlib_cache_archive(
            &destination,
            |destination| {
                std::fs::write(destination, b"new")?;
                Err(AtomicPublicationError::new(
                    destination,
                    PublicationState::Replaced,
                    io::Error::other("injected post-replacement failure"),
                )
                .into())
            },
            1,
            Some("new-key"),
            Some("new-manifest"),
            "new-partition",
            None,
        )
        .expect_err("post-transfer abort");
        assert!(error.to_string().contains("destination replaced"));
        assert!(!destination.exists());
        for extension in ["count", "key", "manifest.json", "partition.json", "sha256"] {
            assert!(!destination.with_extension(extension).exists());
        }
        // The persistent lock inode must never be removed by generation abort.
        assert!(directory.join("stdlib.a.publish.lock").is_file());
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn sidecar_abort_preserves_primary_failure_and_reports_cleanup_failure() {
        let directory = std::env::temp_dir().join(format!(
            "molt-stdlib-sidecar-abort-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create fixture");
        let destination = directory.join("stdlib.a");
        let temporary = directory.join("producer.tmp");
        std::fs::write(&destination, b"old").expect("seed old archive");
        std::fs::write(&temporary, b"new").expect("seed new archive");
        std::fs::create_dir(destination.with_extension("count"))
            .expect("block sidecar publication and cleanup");
        let error = publish_shared_stdlib_cache_archive(
            &destination,
            |destination| {
                molt_artifact_publish::commit_existing_file_atomically(&temporary, destination)
                    .map(|()| None)
                    .map_err(Into::into)
            },
            1,
            Some("new-key"),
            Some("new-manifest"),
            "partition",
            None,
        )
        .expect_err("sidecar failure aborts replaced generation");
        let detail = error.to_string();
        assert!(detail.contains("destination replaced"));
        assert!(detail.contains("prior generation no longer owns this path"));
        assert!(detail.contains("shared stdlib invalidation failed"));
        assert!(!destination.exists());
        assert!(!temporary.exists());
        assert!(destination.with_extension("count").is_dir());
        assert!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<AtomicPublicationError>())
                .is_some(),
            "cleanup must retain typed primary state"
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }
}
