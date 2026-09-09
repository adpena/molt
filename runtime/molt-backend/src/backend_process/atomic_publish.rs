use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions, Permissions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static PUBLICATION_NONCE: AtomicU64 = AtomicU64::new(0);

/// Replacement and durability are separate state transitions. A failed parent
/// directory sync cannot undo a successful replacement or restore its old bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationState {
    Unchanged,
    Replaced,
}

#[derive(Debug)]
pub(crate) struct AtomicPublicationError {
    destination: PathBuf,
    state: PublicationState,
    source: io::Error,
    cleanup_errors: Vec<String>,
}

impl AtomicPublicationError {
    pub(crate) fn new(destination: &Path, state: PublicationState, source: io::Error) -> Self {
        Self {
            destination: destination.to_path_buf(),
            state,
            source,
            cleanup_errors: Vec::new(),
        }
    }

    pub(crate) fn state(&self) -> PublicationState {
        self.state
    }

    pub(crate) fn record_cleanup_error(&mut self, error: io::Error) {
        self.cleanup_errors.push(error.to_string());
    }

    fn remove_temporary(&mut self, temporary: &Path) {
        if let Err(error) = remove_publication_temporary(temporary) {
            self.cleanup_errors.push(format!(
                "temporary cleanup failed for '{}': {error}",
                temporary.display()
            ));
        }
    }
}

impl std::fmt::Display for AtomicPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.state {
            PublicationState::Unchanged => "destination unchanged",
            PublicationState::Replaced => {
                "destination replaced; prior generation no longer owns this path"
            }
        };
        write!(
            formatter,
            "atomic publication of '{}' failed ({state}): {}",
            self.destination.display(),
            self.source
        )?;
        for error in &self.cleanup_errors {
            write!(formatter, "; {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AtomicPublicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl From<AtomicPublicationError> for io::Error {
    fn from(error: AtomicPublicationError) -> Self {
        io::Error::new(error.source.kind(), error)
    }
}

/// Keep cleanup evidence without replacing the primary error or its source
/// chain. Callers own the temporary; this helper never touches the destination.
pub(crate) fn cleanup_temporary_after_error(temporary: &Path, error: io::Error) -> io::Error {
    match remove_publication_temporary(temporary) {
        Ok(()) => error,
        Err(cleanup) => {
            #[derive(Debug)]
            struct CleanupError {
                source: io::Error,
                temporary: PathBuf,
                cleanup: io::Error,
            }
            impl std::fmt::Display for CleanupError {
                fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(
                        formatter,
                        "{}; temporary cleanup failed for '{}': {}",
                        self.source,
                        self.temporary.display(),
                        self.cleanup
                    )
                }
            }
            impl std::error::Error for CleanupError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    Some(&self.source)
                }
            }
            io::Error::new(
                error.kind(),
                CleanupError {
                    source: error,
                    temporary: temporary.to_path_buf(),
                    cleanup,
                },
            )
        }
    }
}

fn remove_publication_temporary(temporary: &Path) -> io::Result<()> {
    match std::fs::remove_file(temporary) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// A same-directory, crash-consistent file publication.
///
/// Data and replacement metadata become visible as one commit: the temporary
/// payload is flushed and synced before replacement, then the containing
/// directory is synced where the platform exposes that durability primitive.
/// Dropping an uncommitted publication removes its private temporary file.
pub(crate) struct AtomicFilePublication {
    destination: PathBuf,
    temporary: PathBuf,
    writer: Option<BufWriter<File>>,
    inherited_permissions: Option<Permissions>,
}

impl AtomicFilePublication {
    pub(crate) fn new(destination: &Path) -> io::Result<Self> {
        let parent = publication_parent(destination);
        std::fs::create_dir_all(parent)?;
        let inherited_permissions = destination_permissions(destination)?;
        let (temporary, file) = reserve_temporary_file(destination)?;
        Ok(Self {
            destination: destination.to_path_buf(),
            temporary,
            writer: Some(BufWriter::new(file)),
            inherited_permissions,
        })
    }

    pub(crate) fn writer(&mut self) -> &mut BufWriter<File> {
        self.writer
            .as_mut()
            .expect("atomic publication writer is unavailable after commit")
    }

    pub(crate) fn commit(self) -> Result<(), AtomicPublicationError> {
        self.commit_with_sync(sync_parent_directory)
    }

    fn commit_with_sync(
        mut self,
        sync_parent: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<(), AtomicPublicationError> {
        let prepared = (|| -> io::Result<()> {
            let mut writer = self
                .writer
                .take()
                .expect("atomic publication cannot be committed twice");
            let flushed = writer.flush();
            // Always dismantle the buffer before propagating a write failure:
            // BufWriter::drop otherwise retries buffered writes during abort.
            let (file, _) = writer.into_parts();
            flushed?;
            if let Some(permissions) = self.inherited_permissions.take() {
                file.set_permissions(permissions)?;
            }
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = prepared {
            return Err(self.abort(error));
        }
        let result = replace_and_sync(&self.temporary, &self.destination, sync_parent);
        if let Err(mut error) = result {
            if error.state() == PublicationState::Unchanged {
                error.remove_temporary(&self.temporary);
            }
            self.temporary.clear();
            return Err(error);
        }
        self.temporary.clear();
        Ok(())
    }

    pub(crate) fn abort(mut self, source: io::Error) -> AtomicPublicationError {
        // Close before unlinking (required on Windows); do not retry a buffered
        // write while aborting. No failed payload may become a publication.
        if let Some(writer) = self.writer.take() {
            let (file, _) = writer.into_parts();
            drop(file);
        }
        let mut error =
            AtomicPublicationError::new(&self.destination, PublicationState::Unchanged, source);
        error.remove_temporary(&self.temporary);
        self.temporary.clear();
        error
    }
}

impl Drop for AtomicFilePublication {
    fn drop(&mut self) {
        if !self.temporary.as_os_str().is_empty() {
            if let Some(writer) = self.writer.take() {
                let (file, _) = writer.into_parts();
                drop(file);
            }
            if let Err(error) = remove_publication_temporary(&self.temporary) {
                // Explicit error paths report through their Result. Drop also
                // covers unwinding and abandonment, where no Result is possible.
                eprintln!(
                    "MOLT_BACKEND: abandoned publication temporary cleanup failed for '{}': {error}",
                    self.temporary.display()
                );
            }
        }
    }
}

pub(crate) fn write_bytes_atomically(destination: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomically(destination, |writer| writer.write_all(bytes))
}

pub(crate) fn write_atomically<T>(
    destination: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> io::Result<T>,
) -> io::Result<T> {
    let mut publication = AtomicFilePublication::new(destination)?;
    let value = match write(publication.writer()) {
        Ok(value) => value,
        Err(error) => return Err(publication.abort(error).into()),
    };
    publication.commit()?;
    Ok(value)
}

#[cfg(any(
    feature = "native-backend",
    feature = "luau-backend",
    feature = "rust-backend",
    test
))]
pub(crate) fn write_text_atomically(destination: &Path, contents: &str) -> io::Result<()> {
    write_bytes_atomically(destination, contents.as_bytes())
}

/// Commit a producer-owned temporary file through the same durability
/// boundary used by direct backend output. The producer must place the file on
/// the destination filesystem so replacement stays atomic.
#[cfg(any(feature = "native-backend", test))]
pub(crate) fn commit_existing_file_atomically(
    temporary: &Path,
    destination: &Path,
) -> Result<(), AtomicPublicationError> {
    commit_existing_file_with_sync(temporary, destination, sync_parent_directory)
}

#[cfg(any(feature = "native-backend", test))]
fn commit_existing_file_with_sync(
    temporary: &Path,
    destination: &Path,
    sync_parent: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), AtomicPublicationError> {
    let prepare = || -> io::Result<()> {
        let parent = publication_parent(destination);
        std::fs::create_dir_all(parent)?;
        if publication_parent(temporary) != publication_parent(destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "atomic publication requires a same-directory temporary file: {} -> {}",
                    temporary.display(),
                    destination.display()
                ),
            ));
        }
        let file = OpenOptions::new().read(true).write(true).open(temporary)?;
        if let Some(permissions) = destination_permissions(destination)? {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        drop(file);
        Ok(())
    };
    prepare().map_err(|error| {
        AtomicPublicationError::new(destination, PublicationState::Unchanged, error)
    })?;
    replace_and_sync(temporary, destination, sync_parent)
}

fn replace_and_sync(
    temporary: &Path,
    destination: &Path,
    sync_parent: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), AtomicPublicationError> {
    replace_file(temporary, destination).map_err(|error| {
        AtomicPublicationError::new(destination, PublicationState::Unchanged, error)
    })?;
    sync_parent(destination).map_err(|error| {
        AtomicPublicationError::new(destination, PublicationState::Replaced, error)
    })
}

fn publication_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn destination_permissions(path: &Path) -> io::Result<Option<Permissions>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.permissions())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn reserve_temporary_file(destination: &Path) -> io::Result<(PathBuf, File)> {
    let file_name = destination.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "atomic publication destination has no file name: {}",
                destination.display()
            ),
        )
    })?;
    let pid = std::process::id();
    for _ in 0..1024 {
        let nonce = PUBLICATION_NONCE.fetch_add(1, Ordering::Relaxed);
        let temporary = destination.with_file_name(temporary_name(file_name, pid, nonce));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "cannot reserve atomic publication beside {} after 1024 attempts",
            destination.display()
        ),
    ))
}

fn temporary_name(file_name: &OsStr, pid: u32, nonce: u64) -> OsString {
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".{pid}.{nonce}.tmp"));
    name
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(unix)]
fn sync_parent_directory(destination: &Path) -> io::Result<()> {
    File::open(publication_parent(destination))?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_destination: &Path) -> io::Result<()> {
    // Windows replacement uses MOVEFILE_WRITE_THROUGH. Other non-Unix targets
    // do not expose a portable directory-sync primitive through std.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        let nonce = PUBLICATION_NONCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "molt-atomic-publication-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create atomic publication test directory");
        path
    }

    #[test]
    fn publication_replaces_existing_payload_without_temporary_residue() {
        let directory = test_directory("replace");
        let output = directory.join("artifact.bin");
        std::fs::write(&output, b"old").expect("seed output");

        write_bytes_atomically(&output, b"new payload").expect("publish output");

        assert_eq!(std::fs::read(&output).expect("read output"), b"new payload");
        assert_eq!(
            std::fs::read_dir(&directory)
                .expect("list test directory")
                .count(),
            1
        );
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn dropped_publication_removes_uncommitted_payload() {
        let directory = test_directory("drop");
        let output = directory.join("artifact.bin");
        {
            let mut publication = AtomicFilePublication::new(&output).expect("reserve output");
            publication
                .writer()
                .write_all(b"partial")
                .expect("write partial payload");
        }

        assert!(!output.exists());
        assert_eq!(
            std::fs::read_dir(&directory)
                .expect("list test directory")
                .count(),
            0
        );
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn existing_temporary_file_uses_the_same_commit_boundary() {
        let directory = test_directory("existing");
        let output = directory.join("artifact.bin");
        let temporary = directory.join(".artifact.bin.producer.tmp");
        std::fs::write(&temporary, b"producer payload").expect("write producer output");

        commit_existing_file_atomically(&temporary, &output).expect("commit producer output");

        assert_eq!(
            std::fs::read(&output).expect("read output"),
            b"producer payload"
        );
        assert!(!temporary.exists());
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn relative_paths_share_the_normalized_current_directory_authority() {
        assert_eq!(
            publication_parent(Path::new("producer.tmp")),
            publication_parent(Path::new("./artifact.bin"))
        );
    }

    #[test]
    fn buffered_publication_reports_post_replacement_sync_failure() {
        let directory = test_directory("buffered-sync-failure");
        let output = directory.join("artifact.bin");
        std::fs::write(&output, b"old").expect("seed old generation");
        let mut publication = AtomicFilePublication::new(&output).expect("prepare publication");
        publication
            .writer()
            .write_all(b"new")
            .expect("write new generation");
        let error = publication
            .commit_with_sync(|_| Err(io::Error::other("injected directory sync failure")))
            .expect_err("post-replacement durability failure");
        assert_eq!(error.state(), PublicationState::Replaced);
        assert!(
            error
                .to_string()
                .contains("prior generation no longer owns this path")
        );
        assert_eq!(
            std::fs::read(&output).expect("replacement survives"),
            b"new"
        );
        assert_eq!(
            std::fs::read_dir(&directory).expect("list fixture").count(),
            1
        );
        let error = io::Error::from(error);
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<AtomicPublicationError>())
                .expect("typed state survives io conversion")
                .state(),
            PublicationState::Replaced
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn existing_file_reports_both_sides_of_replacement_boundary() {
        let directory = test_directory("existing-sync-failure");
        let output = directory.join("artifact.bin");
        let temporary = directory.join("producer.tmp");
        std::fs::write(&output, b"old").expect("seed old generation");
        let before = commit_existing_file_atomically(&temporary, &output)
            .expect_err("missing producer fails before replacement");
        assert_eq!(before.state(), PublicationState::Unchanged);
        assert_eq!(
            std::fs::read(&output).expect("old generation survives"),
            b"old"
        );
        std::fs::write(&temporary, b"new").expect("write producer output");
        let after = commit_existing_file_with_sync(&temporary, &output, |_| {
            Err(io::Error::other("injected directory sync failure"))
        })
        .expect_err("replacement committed but directory sync failed");
        assert_eq!(after.state(), PublicationState::Replaced);
        assert_eq!(
            std::fs::read(&output).expect("new generation survives"),
            b"new"
        );
        assert!(!temporary.exists());
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn failed_payload_write_aborts_without_publishing_or_silent_cleanup() {
        let directory = test_directory("write-failure");
        let output = directory.join("artifact.bin");
        std::fs::write(&output, b"old").expect("seed old generation");
        let error = write_atomically(&output, |writer| -> io::Result<()> {
            writer.write_all(b"uncommitted payload")?;
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "injected producer failure",
            ))
        })
        .expect_err("producer failure propagates");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("destination unchanged"));
        assert_eq!(
            std::fs::read(&output).expect("old generation survives"),
            b"old"
        );
        assert_eq!(
            std::fs::read_dir(&directory).expect("list fixture").count(),
            1
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn failed_temporary_cleanup_keeps_primary_state_and_evidence() {
        let directory = test_directory("cleanup-failure");
        let temporary = directory.join("blocked.tmp");
        std::fs::create_dir(&temporary).expect("unremovable-as-file temporary");
        let primary = AtomicPublicationError::new(
            &directory.join("artifact.bin"),
            PublicationState::Replaced,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "primary durability failure",
            ),
        );
        let error = cleanup_temporary_after_error(&temporary, primary.into());
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let detail = error.to_string();
        assert!(detail.contains("primary durability failure"));
        assert!(detail.contains("destination replaced"));
        assert!(detail.contains("temporary cleanup failed"));
        assert!(detail.contains("blocked.tmp"));
        assert!(std::error::Error::source(error.get_ref().expect("cleanup error")).is_some());
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }
}
