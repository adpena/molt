//! Mount-oriented virtual filesystem for WASM targets.
//!
//! Routes all filesystem operations through a mount table with
//! capability-enforced backends. Falls back to std::fs on native.

pub mod bundle;
pub mod caps;
pub mod dev;
pub mod file;
pub mod snapshot;
pub mod tmp;

use std::sync::{Arc, RwLock};

/// Errors from VFS operations.
#[derive(Debug, Clone)]
pub enum VfsError {
    NotFound,
    PermissionDenied,
    ReadOnly,
    IsDirectory,
    NotDirectory,
    AlreadyExists,
    QuotaExceeded,
    SeekNotSupported,
    IoError(String),
    CapabilityDenied(String),
}

impl std::fmt::Display for VfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "No such file or directory"),
            Self::PermissionDenied => write!(f, "Permission denied"),
            Self::ReadOnly => write!(f, "Read-only file system"),
            Self::IsDirectory => write!(f, "Is a directory"),
            Self::NotDirectory => write!(f, "Not a directory"),
            Self::AlreadyExists => write!(f, "File exists"),
            Self::QuotaExceeded => write!(f, "Quota exceeded"),
            Self::SeekNotSupported => write!(f, "Seek not supported on VFS files"),
            Self::IoError(msg) => write!(f, "{msg}"),
            Self::CapabilityDenied(cap) => write!(f, "missing '{cap}' capability"),
        }
    }
}

/// File/directory metadata.
#[derive(Debug, Clone)]
pub struct VfsStat {
    pub is_file: bool,
    pub is_dir: bool,
    pub size: u64,
    pub readonly: bool,
    pub mtime: u64,
}

/// Backend trait for mount implementations.
pub trait VfsBackend: Send + Sync {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError>;
    fn open_write(&self, path: &str, data: &[u8]) -> Result<(), VfsError>;
    fn open_append(&self, path: &str, data: &[u8]) -> Result<(), VfsError>;
    fn stat(&self, path: &str) -> Result<VfsStat, VfsError>;
    fn readdir(&self, path: &str) -> Result<Vec<String>, VfsError>;
    fn mkdir(&self, path: &str) -> Result<(), VfsError>;
    fn unlink(&self, path: &str) -> Result<(), VfsError>;
    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError>;
    fn exists(&self, path: &str) -> bool;
    fn is_readonly(&self) -> bool;
}

/// Mount table mapping path prefixes to backends.
pub struct MountTable {
    /// Sorted longest-prefix-first for correct resolution.
    mounts: Vec<(String, Arc<dyn VfsBackend>)>,
}

impl Default for MountTable {
    fn default() -> Self {
        Self::new()
    }
}

impl MountTable {
    pub fn new() -> Self {
        Self { mounts: Vec::new() }
    }

    pub fn add_mount(&mut self, prefix: &str, backend: Arc<dyn VfsBackend>) {
        let prefix = prefix.trim_end_matches('/').to_string();
        self.mounts.push((prefix, backend));
        self.mounts.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    }

    /// Resolve a path to (mount_prefix, backend, relative_path).
    /// Returns None if no mount matches.
    pub fn resolve(&self, path: &str) -> Option<(&str, &dyn VfsBackend, String)> {
        let normalized = normalize_path(path)?;
        for (prefix, backend) in &self.mounts {
            if normalized == *prefix
                || (normalized.len() > prefix.len()
                    && normalized.starts_with(prefix.as_str())
                    && normalized.as_bytes()[prefix.len()] == b'/')
            {
                let rel = if normalized.len() == prefix.len() {
                    String::new()
                } else {
                    normalized[prefix.len() + 1..].to_string()
                };
                return Some((prefix.as_str(), backend.as_ref(), rel));
            }
        }
        None
    }
}

/// Normalize a path: collapse //, resolve ., reject .. escapes.
/// Returns None for empty or invalid paths.
fn normalize_path(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let path = if path.starts_with('/') {
        path
    } else {
        return None;
    };

    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                if parts.is_empty() {
                    return None; // traversal escape
                }
                parts.pop();
            }
            other => parts.push(other),
        }
    }

    if parts.is_empty() {
        return None; // bare "/" not allowed
    }

    Some(format!("/{}", parts.join("/")))
}

/// Thread-safe VFS state stored in runtime.
pub struct VfsState {
    pub mount_table: RwLock<MountTable>,
}

impl Default for VfsState {
    fn default() -> Self {
        Self::new()
    }
}

impl VfsState {
    pub fn new() -> Self {
        Self {
            mount_table: RwLock::new(MountTable::new()),
        }
    }

    pub fn resolve(&self, path: &str) -> Option<(String, Arc<dyn VfsBackend>, String)> {
        let table = self.mount_table.read().ok()?;
        let (prefix, _backend, rel) = table.resolve(path)?;
        let prefix = prefix.to_string();
        // Find the Arc to clone
        for (p, backend) in &table.mounts {
            if *p == prefix {
                return Some((prefix, Arc::clone(backend), rel));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::bundle::BundleFs;
    use crate::vfs::dev::DevFs;
    use crate::vfs::file::MoltVfsFile;
    use crate::vfs::tmp::TmpFs;

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_path("").is_none());
    }

    #[test]
    fn normalize_rejects_relative() {
        assert!(normalize_path("foo/bar").is_none());
    }

    #[test]
    fn normalize_rejects_root_escape() {
        // ".." from root should be rejected (can't go above /)
        assert!(normalize_path("/../etc/passwd").is_none());
    }

    #[test]
    fn normalize_resolves_safe_dotdot() {
        // ".." within a path that doesn't escape root is fine
        assert_eq!(
            normalize_path("/bundle/../etc/passwd"),
            Some("/etc/passwd".into())
        );
    }

    #[test]
    fn normalize_rejects_bare_root() {
        assert!(normalize_path("/").is_none());
    }

    #[test]
    fn normalize_collapses_slashes() {
        assert_eq!(
            normalize_path("/bundle//file.txt"),
            Some("/bundle/file.txt".into())
        );
    }

    #[test]
    fn normalize_resolves_dot() {
        assert_eq!(
            normalize_path("/bundle/./file.txt"),
            Some("/bundle/file.txt".into())
        );
    }

    #[test]
    fn mount_table_resolves_longest_prefix() {
        let mut table = MountTable::new();
        let bundle: Arc<dyn VfsBackend> = Arc::new(BundleFs::from_entries(vec![(
            "file.txt".into(),
            b"hello".to_vec(),
        )]));
        let tmp: Arc<dyn VfsBackend> = Arc::new(TmpFs::new(64));
        table.add_mount("/bundle", bundle);
        table.add_mount("/tmp", tmp);

        let (prefix, _, rel) = table.resolve("/bundle/file.txt").unwrap();
        assert_eq!(prefix, "/bundle");
        assert_eq!(rel, "file.txt");

        let (prefix, _, rel) = table.resolve("/tmp/scratch").unwrap();
        assert_eq!(prefix, "/tmp");
        assert_eq!(rel, "scratch");
    }

    #[test]
    fn mount_table_no_match() {
        let table = MountTable::new();
        assert!(table.resolve("/unknown/path").is_none());
    }

    #[test]
    fn bundle_fs_read() {
        let fs = BundleFs::from_entries(vec![("hello.txt".into(), b"world".to_vec())]);
        assert_eq!(fs.open_read("hello.txt").unwrap(), b"world");
        assert!(fs.open_read("missing.txt").is_err());
    }

    #[test]
    fn bundle_fs_readonly() {
        let fs = BundleFs::from_entries(vec![]);
        assert!(fs.open_write("file.txt", b"data").is_err());
        assert!(fs.is_readonly());
    }

    #[test]
    fn bundle_fs_readdir() {
        let fs = BundleFs::from_entries(vec![
            ("a.txt".into(), b"".to_vec()),
            ("sub/b.txt".into(), b"".to_vec()),
        ]);
        let mut entries = fs.readdir("").unwrap();
        entries.sort();
        assert_eq!(entries, vec!["a.txt", "sub"]);
    }

    #[test]
    fn bundle_fs_stat() {
        let fs = BundleFs::from_entries(vec![("data.csv".into(), b"a,b,c".to_vec())]);
        let stat = fs.stat("data.csv").unwrap();
        assert!(stat.is_file);
        assert!(!stat.is_dir);
        assert_eq!(stat.size, 5);
        assert!(stat.readonly);
        assert_eq!(stat.mtime, 0);
    }

    #[test]
    fn tmp_fs_write_read() {
        let fs = TmpFs::new(64);
        fs.open_write("file.txt", b"hello").unwrap();
        assert_eq!(fs.open_read("file.txt").unwrap(), b"hello");
    }

    #[test]
    fn tmp_fs_quota() {
        let fs = TmpFs::new(0);
        assert!(matches!(
            fs.open_write("file.txt", b"data"),
            Err(VfsError::QuotaExceeded)
        ));
    }

    #[test]
    fn tmp_fs_rename() {
        let fs = TmpFs::new(64);
        fs.open_write("a.txt", b"content").unwrap();
        fs.rename("a.txt", "b.txt").unwrap();
        assert!(!fs.exists("a.txt"));
        assert_eq!(fs.open_read("b.txt").unwrap(), b"content");
    }

    #[test]
    fn tmp_fs_unlink() {
        let fs = TmpFs::new(64);
        fs.open_write("file.txt", b"data").unwrap();
        fs.unlink("file.txt").unwrap();
        assert!(!fs.exists("file.txt"));
    }

    #[test]
    fn tmp_fs_mkdir() {
        let fs = TmpFs::new(64);
        fs.mkdir("subdir").unwrap();
        assert!(fs.stat("subdir").unwrap().is_dir);
    }

    #[test]
    fn dev_fs_stdin_read() {
        let mut fs = DevFs::new();
        fs.set_stdin(b"input data".to_vec());
        assert_eq!(fs.open_read("stdin").unwrap(), b"input data");
    }

    #[test]
    fn dev_fs_stdout_write() {
        let fs = DevFs::new();
        fs.open_write("stdout", b"hello ").unwrap();
        fs.open_write("stdout", b"world").unwrap();
        assert_eq!(fs.take_stdout(), b"hello world");
    }

    #[test]
    fn dev_fs_readdir() {
        let fs = DevFs::new();
        let entries = fs.readdir("").unwrap();
        assert!(entries.contains(&"stdin".to_string()));
        assert!(entries.contains(&"stdout".to_string()));
        assert!(entries.contains(&"stderr".to_string()));
    }

    #[test]
    fn vfs_file_read_write_cycle() {
        let backend: Arc<dyn VfsBackend> = Arc::new(TmpFs::new(64));
        let mut f = MoltVfsFile::open_write(Arc::clone(&backend), "test.txt").unwrap();
        f.write(b"hello world").unwrap();
        f.close().unwrap();
        let mut f = MoltVfsFile::open_read(Arc::clone(&backend), "test.txt").unwrap();
        assert_eq!(f.read(5), b"hello");
        assert_eq!(f.read_all(), b" world");
        assert_eq!(f.tell(), 11);
    }

    #[test]
    fn capability_denies_without_grant() {
        use crate::vfs::caps::check_mount_capability;
        let no_caps = |_: &str| false;
        assert!(matches!(
            check_mount_capability("/bundle", false, &no_caps),
            Err(VfsError::CapabilityDenied(_))
        ));
    }

    #[test]
    fn capability_allows_dev_always() {
        use crate::vfs::caps::check_mount_capability;
        let no_caps = |_: &str| false;
        assert!(check_mount_capability("/dev", false, &no_caps).is_ok());
        assert!(check_mount_capability("/dev", true, &no_caps).is_ok());
    }

    #[test]
    fn capability_denies_bundle_write() {
        use crate::vfs::caps::check_mount_capability;
        let all_caps = |_: &str| true;
        assert!(matches!(
            check_mount_capability("/bundle", true, &all_caps),
            Err(VfsError::ReadOnly)
        ));
    }

    #[test]
    fn tmp_fs_quota_enforced_on_sequential_writes() {
        // Write files that together exceed quota
        let fs = TmpFs::new(1); // 1 MB quota
        let data = vec![0u8; 600_000]; // 600 KB
        fs.open_write("a.txt", &data).unwrap();
        let result = fs.open_write("b.txt", &data); // Should fail - 1.2 MB > 1 MB
        // The second write should fail with QuotaExceeded
        assert!(matches!(result, Err(VfsError::QuotaExceeded)));
    }

    #[test]
    fn mount_escape_via_dotdot_resolves_to_correct_mount() {
        // /bundle/../tmp/file should resolve to /tmp mount, not /bundle
        let mut table = MountTable::new();
        let bundle: Arc<dyn VfsBackend> = Arc::new(BundleFs::from_entries(vec![]));
        let tmp: Arc<dyn VfsBackend> = Arc::new(TmpFs::new(64));
        table.add_mount("/bundle", bundle);
        table.add_mount("/tmp", tmp);

        let resolved = table.resolve("/bundle/../tmp/secret");
        assert!(resolved.is_some());
        let (prefix, _, rel) = resolved.unwrap();
        assert_eq!(prefix, "/tmp"); // Resolves to /tmp, not /bundle
        assert_eq!(rel, "secret");
    }

    #[test]
    fn dev_fs_buffer_cap() {
        let fs = DevFs::new();
        let big_data = vec![0u8; 20 * 1024 * 1024]; // 20 MB exceeds 16 MB cap
        let result = fs.open_write("stdout", &big_data);
        assert!(matches!(result, Err(VfsError::QuotaExceeded)));
    }
}
