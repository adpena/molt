use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions, Permissions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static PUBLICATION_NONCE: AtomicU64 = AtomicU64::new(0);

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

    pub(crate) fn commit(mut self) -> io::Result<()> {
        let mut writer = self
            .writer
            .take()
            .expect("atomic publication cannot be committed twice");
        writer.flush()?;
        if let Some(permissions) = self.inherited_permissions.take() {
            writer.get_ref().set_permissions(permissions)?;
        }
        writer.get_ref().sync_all()?;
        drop(writer);
        replace_file(&self.temporary, &self.destination)?;
        sync_parent_directory(&self.destination)?;
        self.temporary.clear();
        Ok(())
    }
}

impl Drop for AtomicFilePublication {
    fn drop(&mut self) {
        if !self.temporary.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.temporary);
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
    let value = write(publication.writer())?;
    publication.commit()?;
    Ok(value)
}

pub(crate) fn write_text_atomically(destination: &Path, contents: &str) -> io::Result<()> {
    write_bytes_atomically(destination, contents.as_bytes())
}

/// Commit a producer-owned temporary file through the same durability
/// boundary used by direct backend output. The producer must place the file on
/// the destination filesystem so replacement stays atomic.
pub(crate) fn commit_existing_file_atomically(
    temporary: &Path,
    destination: &Path,
) -> io::Result<()> {
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
    replace_file(temporary, destination)?;
    sync_parent_directory(destination)
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
}
