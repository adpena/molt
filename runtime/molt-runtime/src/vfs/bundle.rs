//! Read-only in-memory filesystem for /bundle mount.
//! Populated from a tar archive or explicit file entries at init.

use crate::vfs::{VfsBackend, VfsError, VfsStat};
use std::collections::{BTreeMap, BTreeSet};

pub struct BundleFs {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
}

impl BundleFs {
    /// Create from explicit file entries.
    pub fn from_entries(entries: Vec<(String, Vec<u8>)>) -> Self {
        let mut files = BTreeMap::new();
        let mut dirs = BTreeSet::new();
        for (path, content) in entries {
            // Register all parent directories
            let mut parent = String::new();
            for component in path.split('/') {
                if !parent.is_empty() || component.is_empty() {
                    if !parent.is_empty() {
                        dirs.insert(parent.clone());
                    }
                    if !parent.is_empty() {
                        parent.push('/');
                    }
                }
                parent.push_str(component);
            }
            files.insert(path, content);
        }
        dirs.insert(String::new()); // root dir
        Self { files, dirs }
    }

    /// Create from raw tar bytes.
    /// Rejects symlinks and paths containing ".." (traversal protection).
    #[cfg(feature = "vfs_bundle_tar")]
    pub fn from_tar(tar_bytes: &[u8]) -> Result<Self, String> {
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
            // Security: reject traversal
            if path.contains("..") {
                return Err(format!("bundle tar contains '..' in path: {path}"));
            }
            if entry.header().entry_type().is_file() {
                let mut content = Vec::new();
                entry.read_to_end(&mut content).map_err(|e| e.to_string())?;
                entries.push((path, content));
            }
        }
        Ok(Self::from_entries(entries))
    }
}

impl VfsBackend for BundleFs {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        if self.dirs.contains(path) {
            return Err(VfsError::IsDirectory);
        }
        self.files.get(path).cloned().ok_or(VfsError::NotFound)
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
