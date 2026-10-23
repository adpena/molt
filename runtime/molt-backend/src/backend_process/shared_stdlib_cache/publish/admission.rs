use std::{io, path::Path};

use super::files::refresh_cache_entry_recency;
use super::lock::with_shared_stdlib_cache_publish_lock;
use super::paths::stdlib_cache_count_sidecar_path;
use super::sidecars::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, read_stdlib_cache_partition_manifest,
    remove_shared_stdlib_cache_artifacts, shared_stdlib_archive_payload_matches,
    shared_stdlib_cache_matches_unlocked, shared_stdlib_cache_recorded_digest,
};

/// A reader observes the archive and all sidecars from one published generation.
/// Lock failures are loud cache misses; compile admission propagates them below.
#[cfg(any(unix, test))]
pub(crate) fn shared_stdlib_cache_matches(
    path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition: Option<&str>,
) -> bool {
    with_shared_stdlib_cache_publish_lock(path, || {
        Ok(shared_stdlib_cache_matches_unlocked(
            path,
            expected_key,
            expected_manifest,
            expected_partition,
        ))
    })
    .unwrap_or_else(|error| {
        eprintln!(
            "MOLT_BACKEND: shared stdlib admission lock failed for '{}': {error}",
            path.display()
        );
        false
    })
}

/// One archive generation's path and the identity its sidecars record.
#[derive(Clone, Copy)]
pub(crate) struct ArchiveGeneration<'a> {
    pub(crate) path: &'a Path,
    pub(crate) key: Option<&'a str>,
    pub(crate) manifest: Option<&'a str>,
}

/// Transfer an admitted cache generation to `destination`, recording the
/// destination's own identity, and return the archive digest; `None` is a miss.
///
/// The transfer is a platform copy, a copy-on-write clone where the volume
/// supports it, so republishing 67 MB of unchanged archive writes no data. The
/// private copy is verified against the source's recorded digest before it
/// replaces the destination: one read, which also leaves the destination's
/// pages cached for the CLI admission and the linker that read it next. A
/// mismatched payload is a miss, as for the source's own admission. Downstream
/// work never reopens the cache entry.
pub(crate) fn hydrate_shared_stdlib_cache_archive(
    source: ArchiveGeneration<'_>,
    destination: ArchiveGeneration<'_>,
    partition: &str,
    stdlib_count: usize,
) -> io::Result<Option<String>> {
    super::paths::validate_required_stdlib_output(destination.path, Some(source.path))?;
    let resolved_source = super::paths::resolved_archive_path(source.path)?;
    let source_path = resolved_source.as_path();
    with_shared_stdlib_cache_publish_lock(source_path, || {
        let Some(digest) = shared_stdlib_cache_recorded_digest(
            source_path,
            source.key,
            source.manifest,
            Some(partition),
        ) else {
            return Ok(None);
        };
        let mut mismatched = false;
        let published = super::archive::publish_shared_stdlib_cache_archive(
            destination.path,
            |target| {
                molt_artifact_publish::copy_file_atomically(source_path, target, |staged| {
                    if shared_stdlib_archive_payload_matches(staged, &digest) {
                        Ok(())
                    } else {
                        mismatched = true;
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "shared stdlib cache payload disagrees with its recorded digest",
                        ))
                    }
                })
                .map(|()| Some(digest.clone()))
            },
            stdlib_count,
            destination.key,
            destination.manifest,
            partition,
        );
        let published = match published {
            Ok(published) => published,
            // The destination is unchanged; a corrupt source is not reuse.
            Err(_) if mismatched => return Ok(None),
            Err(error) => return Err(error),
        };
        if super::paths::is_stdlib_archive_cache_entry(source_path) {
            refresh_cache_entry_recency(source_path.parent().expect("archive entry owner"));
        }
        Ok(Some(published))
    })
}

/// Keep the validation decision and its invalidation in the same custody
/// critical section. Compilation is intentionally outside this lock.
pub(crate) fn admit_or_invalidate_shared_stdlib_cache(
    path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition: &str,
    current_stdlib_count: usize,
    log_prefix: &str,
) -> io::Result<bool> {
    with_shared_stdlib_cache_publish_lock(path, || {
        if !path.try_exists()? {
            return Ok(false);
        }
        if shared_stdlib_cache_matches_unlocked(
            path,
            expected_key,
            expected_manifest,
            Some(expected_partition),
        ) {
            return Ok(true);
        }
        let cached_key = read_stdlib_cache_key(path);
        let cached_manifest = read_stdlib_cache_manifest(path);
        let cached_partition = read_stdlib_cache_partition_manifest(path);
        let cached_count = std::fs::read_to_string(stdlib_cache_count_sidecar_path(path))
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok());
        eprintln!(
            "{log_prefix}: stdlib cache contract mismatch \
             (cached key {}, expected key {}; cached manifest {}, expected manifest present {}; \
             cached partition manifest present {}, expected partition manifest present true; \
             cached {:?} functions, need {}) -- rebuilding",
            cached_key.as_deref().unwrap_or("<missing>"),
            expected_key.unwrap_or("<missing>"),
            cached_manifest.as_deref().unwrap_or("<missing>"),
            expected_manifest.is_some(),
            cached_partition.is_some(),
            cached_count,
            current_stdlib_count,
        );
        // A miss for a different key does not own that generation. Preserve it
        // until this compilation successfully publishes its replacement.
        if expected_key.filter(|key| !key.is_empty()) == cached_key.as_deref()
            && cached_key.is_some()
        {
            remove_shared_stdlib_cache_artifacts(path)?;
        }
        Ok(false)
    })
}

#[cfg(test)]
mod tests {
    use super::super::sidecars::write_shared_stdlib_cache_sidecars;
    use super::*;
    use crate::backend_process::write_native_archive_bytes;
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        time::Duration,
    };

    static NONCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "molt-stdlib-admission-{}-{}",
                std::process::id(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create admission fixture");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove admission fixture");
        }
    }

    fn seed(path: &Path, key: &str) {
        let mut object = object::write::Object::new(
            object::BinaryFormat::Elf,
            object::Architecture::X86_64,
            object::Endianness::Little,
        );
        object.add_section(Vec::new(), b".text".to_vec(), object::SectionKind::Text);
        write_native_archive_bytes(path, &object.write().expect("fixture object"))
            .expect("fixture archive");
        write_shared_stdlib_cache_sidecars(path, 0, Some(key), Some("manifest"), "partition", None)
            .expect("fixture sidecars");
    }

    #[test]
    fn readers_and_invalidators_observe_only_complete_publication() {
        let directory = TestDirectory::new();
        let path = directory.0.join("stdlib.a");
        seed(&path, "old");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let publisher_path = path.clone();
        let publisher = std::thread::spawn(move || {
            with_shared_stdlib_cache_publish_lock(&publisher_path, || {
                // Deliberately expose a mixed sidecar generation while holding
                // the real publication lock, just as a multi-file commit does.
                std::fs::write(publisher_path.with_extension("key"), "new")?;
                std::fs::write(publisher_path.with_extension("manifest.json"), "pending")?;
                started_tx.send(()).expect("publication started");
                release_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("release publication");
                write_shared_stdlib_cache_sidecars(
                    &publisher_path,
                    0,
                    Some("new"),
                    Some("manifest"),
                    "partition",
                    None,
                )
                .map(drop)
            })
            .expect("publish complete generation");
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("publisher holds custody");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let mut readers = Vec::new();
        for invalidate in [false, true] {
            let reader_path = path.clone();
            let ready = ready_tx.clone();
            let result = result_tx.clone();
            readers.push(std::thread::spawn(move || {
                ready.send(()).expect("reader ready");
                let matched = if invalidate {
                    admit_or_invalidate_shared_stdlib_cache(
                        &reader_path,
                        Some("new"),
                        Some("manifest"),
                        "partition",
                        0,
                        "test",
                    )
                    .expect("locked invalidating admission")
                } else {
                    shared_stdlib_cache_matches(
                        &reader_path,
                        Some("new"),
                        Some("manifest"),
                        Some("partition"),
                    )
                };
                result.send(matched).expect("reader result");
            }));
        }
        for _ in 0..2 {
            ready_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("reader started");
        }
        let premature = result_rx.recv_timeout(Duration::from_millis(50));
        release_tx.send(()).expect("finish publication");
        publisher.join().expect("join publisher");
        for reader in readers {
            reader.join().expect("join reader");
        }
        assert!(
            premature.is_err(),
            "reader admitted a half-published generation"
        );
        for _ in 0..2 {
            assert!(
                result_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("admission result")
            );
        }
        assert!(
            path.exists(),
            "an invalidator removed the publisher-owned archive"
        );
    }

    #[test]
    fn foreign_key_generation_survives_miss_but_owned_corruption_is_evicted() {
        let directory = TestDirectory::new();
        let path = directory.0.join("stdlib.a");
        seed(&path, "owner");
        let previous = std::fs::read(&path).expect("previous bytes");
        assert!(
            !admit_or_invalidate_shared_stdlib_cache(
                &path,
                Some("other"),
                Some("manifest"),
                "partition",
                0,
                "test",
            )
            .expect("foreign generation miss")
        );
        assert_eq!(
            std::fs::read(&path).expect("foreign generation survives"),
            previous
        );
        assert_eq!(read_stdlib_cache_key(&path).as_deref(), Some("owner"));
        std::fs::write(&path, b"corrupt").expect("corrupt owned archive");
        assert!(
            !admit_or_invalidate_shared_stdlib_cache(
                &path,
                Some("owner"),
                Some("manifest"),
                "partition",
                0,
                "test",
            )
            .expect("owned corruption evicted")
        );
        assert!(!path.exists());
        assert!(read_stdlib_cache_key(&path).is_none());
    }

    #[test]
    fn invalidation_io_failures_are_not_successful_cache_misses() {
        let directory = TestDirectory::new();
        let path = directory.0.join("stdlib.a");
        std::fs::create_dir(&path).expect("unremovable-as-file archive");
        std::fs::write(path.with_extension("key"), "owner").expect("owned key");
        let error = admit_or_invalidate_shared_stdlib_cache(
            &path,
            Some("owner"),
            Some("manifest"),
            "partition",
            0,
            "test",
        )
        .expect_err("failed invalidation must propagate");
        assert!(
            error
                .to_string()
                .contains("shared stdlib invalidation failed")
        );
        assert!(path.is_dir());
    }
}
