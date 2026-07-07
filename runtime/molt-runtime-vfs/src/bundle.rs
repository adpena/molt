//! Read-only in-memory filesystem for /bundle mount.
//! Populated from a tar archive or explicit file entries at init.

#[cfg(feature = "vfs_bundle_tar")]
use crate::load::VfsLoadQuota;
use crate::{VfsBackend, VfsError, VfsStat};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug)]
pub struct BundleFs {
    files: BTreeMap<String, Arc<Vec<u8>>>,
    dirs: BTreeSet<String>,
}

impl BundleFs {
    /// Create from explicit file entries.
    ///
    /// Panics when an entry path is not a portable bundle-relative path. Runtime
    /// loading paths should use [`Self::try_from_entries`] so malformed host
    /// input fails closed with a diagnostic instead of panicking.
    pub fn from_entries(entries: Vec<(String, Vec<u8>)>) -> Self {
        Self::try_from_entries(entries).expect("invalid VFS bundle entries")
    }

    /// Create from explicit file entries, rejecting unsafe or ambiguous paths.
    pub fn try_from_entries(entries: Vec<(String, Vec<u8>)>) -> Result<Self, VfsError> {
        let mut files = BTreeMap::new();
        let mut dirs = BTreeSet::new();
        for (path, content) in entries {
            let path = normalize_bundle_entry_path(&path)?;
            if dirs.contains(&path) || files.contains_key(&path) {
                return Err(VfsError::AlreadyExists);
            }
            // Register all parent directories
            let mut parent = String::new();
            for component in path.split('/') {
                if !parent.is_empty() {
                    if files.contains_key(&parent) {
                        return Err(VfsError::NotDirectory);
                    }
                    dirs.insert(parent.clone());
                    parent.push('/');
                }
                parent.push_str(component);
            }
            files.insert(path, Arc::new(content));
        }
        dirs.insert(String::new()); // root dir
        Ok(Self { files, dirs })
    }

    /// Create from raw tar bytes.
    /// Rejects symlinks and paths containing ".." (traversal protection).
    #[cfg(feature = "vfs_bundle_tar")]
    pub fn from_tar(tar_bytes: &[u8]) -> Result<Self, String> {
        let mut quota = VfsLoadQuota::from_env();
        Self::from_tar_with_quota(tar_bytes, &mut quota)
    }

    /// Create from raw tar bytes with cumulative load quota enforcement.
    #[cfg(feature = "vfs_bundle_tar")]
    pub(crate) fn from_tar_with_quota(
        tar_bytes: &[u8],
        quota: &mut VfsLoadQuota,
    ) -> Result<Self, String> {
        use std::io::Read;
        let mut archive = tar::Archive::new(tar_bytes);
        let mut entries = Vec::new();
        for entry in archive.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry
                .path()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .to_string();
            // Security: reject symlinks
            if entry.header().entry_type().is_symlink()
                || entry.header().entry_type().is_hard_link()
            {
                return Err(format!("bundle tar contains symlink: {path}"));
            }
            // Security: reject traversal (component-level check)
            if path.split('/').any(|c| c == "..") {
                return Err(format!(
                    "bundle tar contains '..' component in path: {path}"
                ));
            }
            // Security: reject absolute paths
            if path.starts_with('/') {
                return Err(format!("bundle tar contains absolute path: {path}"));
            }
            if entry.header().entry_type().is_file() {
                let expected_len = usize::try_from(entry.size())
                    .map_err(|_| format!("bundle tar entry too large: {path}"))?;
                quota
                    .reserve_entry(&path, expected_len)
                    .map_err(|e| e.to_string())?;
                let mut content = Vec::new();
                entry.read_to_end(&mut content).map_err(|e| e.to_string())?;
                if content.len() > expected_len {
                    quota
                        .reserve_additional_bytes(content.len() - expected_len)
                        .map_err(|e| e.to_string())?;
                }
                entries.push((path, content));
            }
        }
        Self::try_from_entries(entries).map_err(|e| e.to_string())
    }
}

fn normalize_bundle_entry_path(path: &str) -> Result<String, VfsError> {
    if path.is_empty() || path.contains('\0') {
        return Err(VfsError::IoError("invalid bundle entry path".to_string()));
    }
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(VfsError::IoError("absolute bundle entry path".to_string()));
    }

    let mut parts = Vec::new();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(VfsError::IoError(format!(
                "invalid bundle entry path component: {path}"
            )));
        }
        if is_windows_drive_component(component) {
            return Err(VfsError::IoError(format!(
                "bundle entry path must be relative: {path}"
            )));
        }
        parts.push(component);
    }
    if parts.is_empty() {
        return Err(VfsError::IoError("invalid bundle entry path".to_string()));
    }
    Ok(parts.join("/"))
}

fn is_windows_drive_component(component: &str) -> bool {
    let bytes = component.as_bytes();
    bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

impl VfsBackend for BundleFs {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        if self.dirs.contains(path) {
            return Err(VfsError::IsDirectory);
        }
        self.files
            .get(path)
            .map(|arc| (**arc).clone())
            .ok_or(VfsError::NotFound)
    }

    fn open_read_shared(&self, path: &str) -> Result<Arc<Vec<u8>>, VfsError> {
        if self.dirs.contains(path) {
            return Err(VfsError::IsDirectory);
        }
        self.files
            .get(path)
            .map(Arc::clone)
            .ok_or(VfsError::NotFound)
    }

    fn open_write(&self, _path: &str, _data: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn open_append(&self, _path: &str, _data: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn stat(&self, path: &str) -> Result<VfsStat, VfsError> {
        if let Some(content) = self.files.get(path) {
            return Ok(VfsStat {
                is_file: true,
                is_dir: false,
                size: content.len() as u64,
                readonly: true,
                mtime: 0,
            });
        }
        if self.dirs.contains(path) {
            return Ok(VfsStat {
                is_file: false,
                is_dir: true,
                size: 0,
                readonly: true,
                mtime: 0,
            });
        }
        Err(VfsError::NotFound)
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, VfsError> {
        if !self.dirs.contains(path) {
            return Err(if self.files.contains_key(path) {
                VfsError::NotDirectory
            } else {
                VfsError::NotFound
            });
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for key in self.files.keys().chain(self.dirs.iter()) {
            if let Some(rest) = key.strip_prefix(&prefix)
                && let Some(name) = rest.split('/').next()
                && !name.is_empty()
                && seen.insert(name.to_string())
            {
                entries.push(name.to_string());
            }
        }
        entries.sort();
        Ok(entries)
    }

    fn mkdir(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn rename(&self, _from: &str, _to: &str) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn exists(&self, path: &str) -> bool {
        self.files.contains_key(path) || self.dirs.contains(path)
    }

    fn is_readonly(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_entries_reject_absolute_and_traversal_paths() {
        for path in [
            "/abs.txt",
            "dir/../secret.txt",
            "dir/./file.txt",
            "dir//file.txt",
            "C:/secret.txt",
            "dir/\0/file.txt",
        ] {
            let err = BundleFs::try_from_entries(vec![(path.to_string(), b"x".to_vec())])
                .expect_err(path);
            assert!(matches!(err, VfsError::IoError(_)), "{path}: {err:?}");
        }
    }

    #[test]
    fn bundle_entries_reject_duplicate_normalized_paths() {
        let err = BundleFs::try_from_entries(vec![
            ("dir/file.txt".to_string(), b"a".to_vec()),
            ("dir\\file.txt".to_string(), b"b".to_vec()),
        ])
        .expect_err("duplicate normalized bundle path should fail");
        assert!(matches!(err, VfsError::AlreadyExists));
    }

    #[test]
    fn bundle_entries_reject_file_directory_collisions() {
        let err = BundleFs::try_from_entries(vec![
            ("dir".to_string(), b"file".to_vec()),
            ("dir/nested.txt".to_string(), b"nested".to_vec()),
        ])
        .expect_err("file cannot later become directory");
        assert!(matches!(err, VfsError::NotDirectory));

        let err = BundleFs::try_from_entries(vec![
            ("dir/nested.txt".to_string(), b"nested".to_vec()),
            ("dir".to_string(), b"file".to_vec()),
        ])
        .expect_err("directory cannot later become file");
        assert!(matches!(err, VfsError::AlreadyExists));
    }

    #[test]
    fn bundle_entries_normalize_host_separators_to_portable_paths() {
        let fs = BundleFs::try_from_entries(vec![("dir\\file.txt".to_string(), b"x".to_vec())])
            .expect("portable normalized path");
        assert_eq!(fs.open_read("dir/file.txt").unwrap(), b"x");
        assert!(fs.exists("dir"));
    }
}
