//! File identity: whether two paths name the same file, decided by the file
//! system, never by comparing path text.
//!
//! CPython's `os.path.samefile` (`Lib/genericpath.py`, `samestat`) compares
//! `(st_dev, st_ino)`. POSIX reports that pair from `stat`; on Windows CPython
//! reads it from the volume serial number and the 128-bit file ID
//! (`Python/fileutils.c`, `_Py_attribute_data_to_stat`).

use std::path::Path;

/// The file system's name for one file: its device (or volume) and its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    device: u64,
    index: u128,
}

impl FileIdentity {
    /// The identity of the file `path` names, following symbolic links.
    pub fn of_path(path: &Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(path)?;
            Ok(Self {
                device: metadata.dev(),
                index: u128::from(metadata.ino()),
            })
        }
        #[cfg(windows)]
        {
            let file = crate::windows_abi::open_metadata_file(path, true)?;
            let facts = crate::windows_abi::file_identity_facts(&file)?;
            Ok(Self {
                device: facts.volume_serial,
                index: facts.file_id,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
        }
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::FileIdentity;

    #[test]
    fn identity_follows_the_file_not_the_spelling() {
        let dir = std::env::temp_dir().join(format!("molt-file-identity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first");
        let second = dir.join("second");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"a").unwrap();
        let spelled_again = dir.join(".").join("first");
        let identity = FileIdentity::of_path(&first).unwrap();
        // Another spelling of the same file has its identity; an equal copy does not.
        assert_eq!(FileIdentity::of_path(&spelled_again).unwrap(), identity);
        assert_ne!(FileIdentity::of_path(&second).unwrap(), identity);
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&first, &link).unwrap();
            assert_eq!(FileIdentity::of_path(&link).unwrap(), identity);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
