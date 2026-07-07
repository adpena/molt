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

use std::borrow::Cow;
use std::sync::{Arc, RwLock};

mod load;
pub use load::{load_vfs, molt_vfs_inject_entry, molt_vfs_inject_finish};

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

    /// Return file contents as a shared `Arc`, avoiding a full copy when the
    /// backend already stores data behind an `Arc`.  The default implementation
    /// wraps the result of [`open_read`] in a new `Arc`.
    fn open_read_shared(&self, path: &str) -> Result<Arc<Vec<u8>>, VfsError> {
        self.open_read(path).map(Arc::new)
    }

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
        self.mounts
            .sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
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
fn normalize_path(path: &str) -> Option<Cow<'_, str>> {
    if path.contains('\0') {
        return None;
    }
    if path.is_empty() || !path.starts_with('/') {
        return None;
    }

    // Fast path: if the path has no problematic sequences, return as-is (zero-alloc).
    if path.len() > 1
        && !path.contains("//")
        && !path.contains("/./")
        && !path.contains("/../")
        && !path.ends_with("/.")
        && !path.ends_with("/..")
    {
        return Some(Cow::Borrowed(path));
    }

    // Slow path: full normalization
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

    Some(Cow::Owned(format!("/{}", parts.join("/"))))
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

    /// Create a `VfsState` from a pre-configured `MountTable`.
    pub fn from_table(table: MountTable) -> Self {
        Self {
            mount_table: RwLock::new(table),
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
    use crate::bundle::BundleFs;
    use crate::dev::DevFs;
    use crate::file::MoltVfsFile;
    use crate::load::{VfsLoadQuota, load_vfs_inner, read_dir_recursive};
    use crate::tmp::TmpFs;
    use std::sync::{MutexGuard, OnceLock};

    fn vfs_global_test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        let guard = LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        unsafe {
            std::env::remove_var("MOLT_VFS_BUNDLE");
            std::env::remove_var("MOLT_VFS_TMP_QUOTA_MB");
            std::env::remove_var("MOLT_VFS_BUNDLE_MAX_BYTES");
            std::env::remove_var("MOLT_VFS_BUNDLE_MAX_ENTRIES");
            std::env::remove_var("MOLT_VFS_BUNDLE_MAX_PATH_BYTES");
            std::env::remove_var("MOLT_VFS_BUNDLE_MAX_ENTRY_BYTES");
            std::env::remove_var("MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS");
        }
        let _ = load_vfs_inner();
        guard
    }

    #[cfg(unix)]
    fn create_test_file_symlink(
        target: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(unix)]
    fn create_test_dir_symlink(
        target: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_test_file_symlink(
        target: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(windows)]
    fn create_test_dir_symlink(
        target: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

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
    fn bundle_fs_read_shared() {
        let fs = BundleFs::from_entries(vec![("hello.txt".into(), b"world".to_vec())]);
        let arc1 = fs.open_read_shared("hello.txt").unwrap();
        let arc2 = fs.open_read_shared("hello.txt").unwrap();
        assert_eq!(&**arc1, b"world");
        // Both Arcs point to the same allocation.
        assert!(Arc::ptr_eq(&arc1, &arc2));
        assert!(fs.open_read_shared("missing.txt").is_err());
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
    fn vfs_load_quota_rejects_cumulative_bytes() {
        let mut quota = VfsLoadQuota::new_for_test(5, 10, 100);
        quota.reserve_entry("a.txt", 3).unwrap();
        assert!(matches!(
            quota.reserve_entry("b.txt", 3),
            Err(VfsError::QuotaExceeded)
        ));
    }

    #[test]
    fn vfs_load_quota_rejects_entry_count() {
        let mut quota = VfsLoadQuota::new_for_test(100, 1, 100);
        quota.reserve_entry("a.txt", 1).unwrap();
        assert!(matches!(
            quota.reserve_entry("b.txt", 1),
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
        use crate::caps::check_mount_capability;
        let no_caps = |_: &str| false;
        assert!(matches!(
            check_mount_capability("/bundle", false, &no_caps),
            Err(VfsError::CapabilityDenied(_))
        ));
    }

    #[test]
    fn capability_allows_dev_always() {
        use crate::caps::check_mount_capability;
        let no_caps = |_: &str| false;
        assert!(check_mount_capability("/dev", false, &no_caps).is_ok());
        assert!(check_mount_capability("/dev", true, &no_caps).is_ok());
    }

    #[test]
    fn capability_denies_bundle_write() {
        use crate::caps::check_mount_capability;
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

    #[test]
    fn vfs_state_from_table() {
        let mut mt = MountTable::new();
        let bundle: Arc<dyn VfsBackend> = Arc::new(BundleFs::from_entries(vec![(
            "main.py".into(),
            b"print('hi')".to_vec(),
        )]));
        mt.add_mount("/bundle", bundle);
        mt.add_mount("/tmp", Arc::new(TmpFs::new(8)));
        mt.add_mount("/dev", Arc::new(DevFs::new()));

        let state = VfsState::from_table(mt);
        let (prefix, backend, rel) = state.resolve("/bundle/main.py").unwrap();
        assert_eq!(prefix, "/bundle");
        assert_eq!(rel, "main.py");
        assert_eq!(backend.open_read("main.py").unwrap(), b"print('hi')");
    }

    #[test]
    fn load_vfs_returns_none_without_env() {
        let _guard = vfs_global_test_lock();
        // When MOLT_VFS_BUNDLE is not set, load_vfs must return None.
        unsafe { std::env::remove_var("MOLT_VFS_BUNDLE") };
        assert!(super::load_vfs().is_none());
    }

    #[test]
    fn load_vfs_from_directory() {
        let _guard = vfs_global_test_lock();
        let dir = std::env::temp_dir().join("molt_vfs_test_load_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("hello.txt"), b"world").unwrap();
        std::fs::write(dir.join("sub/nested.txt"), b"deep").unwrap();

        unsafe { std::env::set_var("MOLT_VFS_BUNDLE", dir.to_str().unwrap()) };
        let state = super::load_vfs().expect("load_vfs should return Some for a valid dir");

        // bundle mount should contain the files
        let (_pfx, backend, rel) = state.resolve("/bundle/hello.txt").unwrap();
        assert_eq!(backend.open_read(&rel).unwrap(), b"world");

        let (_pfx, backend, rel) = state.resolve("/bundle/sub/nested.txt").unwrap();
        assert_eq!(backend.open_read(&rel).unwrap(), b"deep");

        // /tmp and /dev should also be mounted
        assert!(state.resolve("/tmp/anything").is_some());
        assert!(state.resolve("/dev/stdout").is_some());

        // cleanup
        unsafe { std::env::remove_var("MOLT_VFS_BUNDLE") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn injected_bundle_loads_once_for_wasm_hosts() {
        let _guard = vfs_global_test_lock();
        let path = b"dir/hello.txt";
        let data = b"from-wasm-host";

        unsafe {
            super::molt_vfs_inject_entry(path.as_ptr(), path.len(), data.as_ptr(), data.len());
        }

        assert_eq!(super::molt_vfs_inject_finish(), 1);
        let state = super::load_vfs().expect("injected bundle should load");
        let (prefix, backend, rel) = state.resolve("/bundle/dir/hello.txt").unwrap();
        assert_eq!(prefix, "/bundle");
        assert_eq!(rel, "dir/hello.txt");
        assert_eq!(backend.open_read(&rel).unwrap(), data.to_vec());
        assert!(state.resolve("/tmp/scratch").is_some());
        assert!(state.resolve("/dev/stdout").is_some());
        assert!(super::load_vfs().is_none());
    }

    #[test]
    fn injected_bundle_rejects_browser_host_path_escape() {
        let _guard = vfs_global_test_lock();
        let path = b"../secret.txt";
        let data = b"nope";

        unsafe {
            super::molt_vfs_inject_entry(path.as_ptr(), path.len(), data.as_ptr(), data.len());
        }

        assert_eq!(super::molt_vfs_inject_finish(), -1);
        assert!(matches!(
            load_vfs_inner(),
            Err(VfsError::IoError(message)) if message == "invalid injected VFS path"
        ));
        assert!(super::load_vfs().is_none());
    }

    #[test]
    fn read_dir_recursive_collects_files() {
        let dir = std::env::temp_dir().join("molt_vfs_test_readdir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("top.txt"), b"T").unwrap();
        std::fs::write(dir.join("a/mid.txt"), b"M").unwrap();
        std::fs::write(dir.join("a/b/bot.txt"), b"B").unwrap();

        let mut quota = VfsLoadQuota::new_for_test(1024, 8, 1024);
        let entries = read_dir_recursive(dir.to_str().unwrap(), &mut quota).unwrap();
        assert_eq!(entries.len(), 3);

        let map: std::collections::HashMap<String, Vec<u8>> = entries.into_iter().collect();
        assert_eq!(map.get("top.txt").unwrap(), b"T");
        assert_eq!(map.get("a/mid.txt").unwrap(), b"M");
        assert_eq!(map.get("a/b/bot.txt").unwrap(), b"B");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_dir_recursive_enforces_entry_count_quota() {
        let dir = std::env::temp_dir().join("molt_vfs_test_readdir_quota");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"A").unwrap();
        std::fs::write(dir.join("b.txt"), b"B").unwrap();

        let mut quota = VfsLoadQuota::new_for_test(1024, 1, 1024);
        let result = read_dir_recursive(dir.to_str().unwrap(), &mut quota);
        assert!(matches!(result, Err(VfsError::QuotaExceeded)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_dir_recursive_rejects_loose_unsafe_opt_in_values() {
        let _guard = vfs_global_test_lock();
        let dir = std::env::temp_dir().join("molt_vfs_test_readdir_loose_unsafe");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("top.txt"), b"T").unwrap();

        unsafe {
            std::env::set_var("MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS", "true");
        }
        let mut quota = VfsLoadQuota::new_for_test(1024, 8, 1024);
        let err = read_dir_recursive(dir.to_str().unwrap(), &mut quota)
            .expect_err("unsafe VFS access must require an exact opt-in value");
        assert!(matches!(
            err,
            VfsError::IoError(message)
                if message.contains("MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS")
                    && message.contains("exactly 1")
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_dir_recursive_rejects_host_links_by_default() {
        let _guard = vfs_global_test_lock();
        let dir = std::env::temp_dir().join("molt_vfs_test_readdir_symlink_reject");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.txt");
        let link = dir.join("linked.txt");
        std::fs::write(&target, b"target").unwrap();
        if create_test_file_symlink(&target, &link).is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let mut quota = VfsLoadQuota::new_for_test(1024, 8, 1024);
        let err = read_dir_recursive(dir.to_str().unwrap(), &mut quota)
            .expect_err("directory bundle loading must reject host links by default");
        assert!(matches!(
            err,
            VfsError::IoError(message) if message.contains("bundle directory contains host link")
                && message.contains("linked.txt")
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_dir_recursive_rejects_host_link_root_by_default() {
        let _guard = vfs_global_test_lock();
        let target_dir = std::env::temp_dir().join("molt_vfs_test_readdir_root_target");
        let link_dir = std::env::temp_dir().join("molt_vfs_test_readdir_root_link");
        let _ = std::fs::remove_dir_all(&target_dir);
        let _ = std::fs::remove_dir_all(&link_dir);
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("target.txt"), b"target").unwrap();
        if create_test_dir_symlink(&target_dir, &link_dir).is_err() {
            let _ = std::fs::remove_dir_all(&target_dir);
            return;
        }

        let mut quota = VfsLoadQuota::new_for_test(1024, 8, 1024);
        let err = read_dir_recursive(link_dir.to_str().unwrap(), &mut quota)
            .expect_err("directory bundle root links must be explicit unsafe opt-in");
        assert!(matches!(
            err,
            VfsError::IoError(message) if message.contains("bundle directory root is host link")
        ));

        let _ = std::fs::remove_dir_all(&link_dir);
        let _ = std::fs::remove_dir_all(&target_dir);
    }

    #[test]
    fn read_dir_recursive_follows_host_links_only_with_unsafe_opt_in() {
        let _guard = vfs_global_test_lock();
        let dir = std::env::temp_dir().join("molt_vfs_test_readdir_symlink_opt_in");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.txt");
        let link = dir.join("linked.txt");
        std::fs::write(&target, b"target").unwrap();
        if create_test_file_symlink(&target, &link).is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        unsafe {
            std::env::set_var("MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS", "1");
        }
        let mut quota = VfsLoadQuota::new_for_test(1024, 8, 1024);
        let entries = read_dir_recursive(dir.to_str().unwrap(), &mut quota).unwrap();
        let map: std::collections::HashMap<String, Vec<u8>> = entries.into_iter().collect();
        assert_eq!(map.get("linked.txt").unwrap(), b"target");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
