//! Ephemeral read-write in-memory filesystem for /tmp mount.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use crate::vfs::{VfsBackend, VfsError, VfsStat};

pub struct TmpFs {
    files: RwLock<BTreeMap<String, Vec<u8>>>,
    dirs: RwLock<BTreeSet<String>>,
    quota_bytes: usize,
    used_bytes: AtomicUsize,
}

impl TmpFs {
    pub fn new(quota_mb: usize) -> Self {
        let mut dirs = BTreeSet::new();
        dirs.insert(String::new()); // root
        Self {
            files: RwLock::new(BTreeMap::new()),
            dirs: RwLock::new(dirs),
            quota_bytes: quota_mb * 1024 * 1024,
            used_bytes: AtomicUsize::new(0),
        }
    }

    fn check_quota(&self, additional: usize) -> Result<(), VfsError> {
        let current = self.used_bytes.load(Ordering::Relaxed);
        if current + additional > self.quota_bytes {
            return Err(VfsError::QuotaExceeded);
        }
        Ok(())
    }
}

impl VfsBackend for TmpFs {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let dirs = self.dirs.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        if dirs.contains(path) {
            return Err(VfsError::IsDirectory);
        }
        let files = self.files.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        files.get(path).cloned().ok_or(VfsError::NotFound)
    }

    fn open_write(&self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        let dirs = self.dirs.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        if dirs.contains(path) {
            return Err(VfsError::IsDirectory);
        }
        drop(dirs);

        let mut files = self.files.write().map_err(|e| VfsError::IoError(e.to_string()))?;
        let old_size = files.get(path).map(|v| v.len()).unwrap_or(0);
        if data.len() > old_size {
            self.check_quota(data.len() - old_size)?;
        }
        self.used_bytes.fetch_add(data.len(), Ordering::Relaxed);
        if old_size > 0 {
            self.used_bytes.fetch_sub(old_size, Ordering::Relaxed);
        }
        files.insert(path.to_string(), data.to_vec());
        Ok(())
    }

    fn open_append(&self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        self.check_quota(data.len())?;
        let mut files = self.files.write().map_err(|e| VfsError::IoError(e.to_string()))?;
        let entry = files.entry(path.to_string()).or_default();
        entry.extend_from_slice(data);
        self.used_bytes.fetch_add(data.len(), Ordering::Relaxed);
        Ok(())
    }

    fn stat(&self, path: &str) -> Result<VfsStat, VfsError> {
        let files = self.files.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        if let Some(content) = files.get(path) {
            return Ok(VfsStat {
                is_file: true,
                is_dir: false,
                size: content.len() as u64,
                readonly: false,
                mtime: 0, // no clock on freestanding; host can override
            });
        }
        let dirs = self.dirs.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        if dirs.contains(path) {
            return Ok(VfsStat {
                is_file: false,
                is_dir: true,
                size: 0,
                readonly: false,
                mtime: 0,
            });
        }
        Err(VfsError::NotFound)
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, VfsError> {
        let dirs = self.dirs.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        if !dirs.contains(path) {
            let files = self.files.read().map_err(|e| VfsError::IoError(e.to_string()))?;
            return Err(if files.contains_key(path) {
                VfsError::NotDirectory
            } else {
                VfsError::NotFound
            });
        }
        let prefix = if path.is_empty() { String::new() } else { format!("{path}/") };
        let files = self.files.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        let mut entries = std::collections::HashSet::new();
        for key in files.keys().chain(dirs.iter()) {
            if let Some(rest) = key.strip_prefix(&prefix) {
                if let Some(name) = rest.split('/').next() {
                    if !name.is_empty() {
                        entries.insert(name.to_string());
                    }
                }
            }
        }
        let mut sorted: Vec<String> = entries.into_iter().collect();
        sorted.sort();
        Ok(sorted)
    }

    fn mkdir(&self, path: &str) -> Result<(), VfsError> {
        let files = self.files.read().map_err(|e| VfsError::IoError(e.to_string()))?;
        if files.contains_key(path) {
            return Err(VfsError::AlreadyExists);
        }
        drop(files);
        let mut dirs = self.dirs.write().map_err(|e| VfsError::IoError(e.to_string()))?;
        if !dirs.insert(path.to_string()) {
            return Err(VfsError::AlreadyExists);
        }
        Ok(())
    }

    fn unlink(&self, path: &str) -> Result<(), VfsError> {
        let mut files = self.files.write().map_err(|e| VfsError::IoError(e.to_string()))?;
        if let Some(content) = files.remove(path) {
            self.used_bytes.fetch_sub(content.len(), Ordering::Relaxed);
            return Ok(());
        }
        Err(VfsError::NotFound)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let mut files = self.files.write().map_err(|e| VfsError::IoError(e.to_string()))?;
        let content = files.remove(from).ok_or(VfsError::NotFound)?;
        files.insert(to.to_string(), content);
        Ok(())
    }

    fn exists(&self, path: &str) -> bool {
        let files = self.files.read().unwrap_or_else(|e| e.into_inner());
        if files.contains_key(path) {
            return true;
        }
        let dirs = self.dirs.read().unwrap_or_else(|e| e.into_inner());
        dirs.contains(path)
    }

    fn is_readonly(&self) -> bool {
        false
    }
}
