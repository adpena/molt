# WASM VFS Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the mount-oriented virtual filesystem core in molt-runtime that enables `open()`, `pathlib`, and `import` to work with `/bundle`, `/tmp`, and `/dev` mounts on WASM targets.

**Architecture:** A `VfsBackend` trait with concrete implementations (BundleFs, TmpFs, DevFs) managed by a `MountTable`. All filesystem operations route through the VFS when active (WASM builds), falling back to `std::fs` for native builds. Capabilities are enforced at every mount operation.

**Tech Stack:** Rust (molt-runtime), tar crate for bundle parsing, existing capability system (`has_capability` in channels.rs)

**Spec:** `docs/superpowers/specs/2026-03-20-wasm-vfs-design.md` (Layer 1)

---

## File Structure

### New Files
- `runtime/molt-runtime/src/vfs/mod.rs` — VfsBackend trait, VfsError, VfsStat, MountTable, path resolution
- `runtime/molt-runtime/src/vfs/bundle.rs` — BundleFs: read-only in-memory filesystem
- `runtime/molt-runtime/src/vfs/tmp.rs` — TmpFs: ephemeral read-write with quota
- `runtime/molt-runtime/src/vfs/dev.rs` — DevFs: /dev/stdin, /dev/stdout, /dev/stderr
- `runtime/molt-runtime/src/vfs/caps.rs` — Mount-to-capability mapping constants
- `runtime/molt-runtime/src/vfs/file.rs` — MoltVfsFile: file handle bridge
- `tests/test_wasm_vfs.py` — Python integration tests for VFS

### Modified Files
- `runtime/molt-runtime/src/lib.rs` — Add `pub mod vfs;`, add VFS to runtime state
- `runtime/molt-runtime/src/builtins/io.rs:3678` — Route `open()` through VFS on WASM
- `runtime/molt-runtime/src/builtins/modules.rs:1082-1135` — Module resolution via VFS
- `runtime/molt-runtime/Cargo.toml` — Add `tar` dependency (optional, for bundle parsing)

---

### Task 1: VfsBackend Trait and VfsError

**Files:**
- Create: `runtime/molt-runtime/src/vfs/mod.rs`
- Modify: `runtime/molt-runtime/src/lib.rs`

- [ ] **Step 1: Create vfs module directory**

```bash
mkdir -p runtime/molt-runtime/src/vfs
```

- [ ] **Step 2: Write VfsBackend trait, VfsError, VfsStat, and MountTable**

Create `runtime/molt-runtime/src/vfs/mod.rs`:

```rust
//! Mount-oriented virtual filesystem for WASM targets.
//!
//! Routes all filesystem operations through a mount table with
//! capability-enforced backends. Falls back to std::fs on native.

pub mod bundle;
pub mod caps;
pub mod dev;
pub mod file;
pub mod tmp;

use std::collections::HashMap;
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
            if normalized == *prefix || normalized.starts_with(&format!("{prefix}/")) {
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
    let path = if path.starts_with('/') { path } else { return None };

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
```

- [ ] **Step 3: Register vfs module in lib.rs**

Add to `runtime/molt-runtime/src/lib.rs` after the existing module declarations:

```rust
pub mod vfs;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`
Expected: Compiles (with warnings about unused modules — submodules don't exist yet)

- [ ] **Step 5: Commit**

```bash
git add runtime/molt-runtime/src/vfs/mod.rs runtime/molt-runtime/src/lib.rs
git commit -m "feat(vfs): add VfsBackend trait, VfsError, MountTable with path resolution"
```

---

### Task 2: Capability Mapping

**Files:**
- Create: `runtime/molt-runtime/src/vfs/caps.rs`

- [ ] **Step 1: Write capability mapping**

Create `runtime/molt-runtime/src/vfs/caps.rs`:

```rust
//! Mount-to-capability mapping for VFS access control.

use crate::vfs::VfsError;

/// (mount_prefix, read_capability, write_capability)
/// Empty string means: no capability required (always allowed) for reads,
/// or never writable for writes.
const MOUNT_CAPABILITIES: &[(&str, &str, &str)] = &[
    ("/bundle", "fs.bundle.read", ""),
    ("/tmp", "fs.tmp.read", "fs.tmp.write"),
    ("/state", "fs.state.read", "fs.state.write"),
    ("/dev", "", ""),
];

/// Check whether the given operation is allowed on the mount.
/// Returns Ok(()) if allowed, Err with diagnostic if denied.
pub fn check_mount_capability(
    mount_prefix: &str,
    is_write: bool,
    has_cap: &dyn Fn(&str) -> bool,
) -> Result<(), VfsError> {
    let entry = MOUNT_CAPABILITIES
        .iter()
        .find(|(prefix, _, _)| mount_prefix.starts_with(prefix));

    let Some((_, read_cap, write_cap)) = entry else {
        return Err(VfsError::NotFound);
    };

    if is_write {
        if write_cap.is_empty() {
            return Err(VfsError::ReadOnly);
        }
        if !has_cap(write_cap) {
            return Err(VfsError::CapabilityDenied(format!(
                "operation requires '{write_cap}' capability\n  \
                 mount: {mount_prefix}\n  \
                 hint: set MOLT_CAPABILITIES={write_cap} or add to host profile"
            )));
        }
    } else {
        if !read_cap.is_empty() && !has_cap(read_cap) {
            return Err(VfsError::CapabilityDenied(format!(
                "operation requires '{read_cap}' capability\n  \
                 mount: {mount_prefix}\n  \
                 hint: set MOLT_CAPABILITIES={read_cap} or add to host profile"
            )));
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`
Expected: Compiles

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/vfs/caps.rs
git commit -m "feat(vfs): add mount-to-capability mapping with diagnostic messages"
```

---

### Task 3: BundleFs (Read-Only In-Memory)

**Files:**
- Create: `runtime/molt-runtime/src/vfs/bundle.rs`

- [ ] **Step 1: Write BundleFs implementation**

Create `runtime/molt-runtime/src/vfs/bundle.rs`:

```rust
//! Read-only in-memory filesystem for /bundle mount.
//! Populated from a tar archive or explicit file entries at init.

use std::collections::{BTreeMap, BTreeSet};
use crate::vfs::{VfsBackend, VfsError, VfsStat};

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
            let path = entry.path().map_err(|e| e.to_string())?
                .to_string_lossy().to_string();
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
            if let Some(rest) = key.strip_prefix(&prefix) {
                if let Some(name) = rest.split('/').next() {
                    if !name.is_empty() && seen.insert(name.to_string()) {
                        entries.push(name.to_string());
                    }
                }
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
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/vfs/bundle.rs
git commit -m "feat(vfs): add BundleFs read-only in-memory filesystem"
```

---

### Task 4: TmpFs (Ephemeral Read-Write)

**Files:**
- Create: `runtime/molt-runtime/src/vfs/tmp.rs`

- [ ] **Step 1: Write TmpFs implementation**

Create `runtime/molt-runtime/src/vfs/tmp.rs`:

```rust
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
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/vfs/tmp.rs
git commit -m "feat(vfs): add TmpFs ephemeral read-write filesystem with quota"
```

---

### Task 5: DevFs (Pseudo-Devices)

**Files:**
- Create: `runtime/molt-runtime/src/vfs/dev.rs`

- [ ] **Step 1: Write DevFs implementation**

Create `runtime/molt-runtime/src/vfs/dev.rs`:

```rust
//! Pseudo-device filesystem for /dev/stdin, /dev/stdout, /dev/stderr.

use std::sync::Mutex;
use crate::vfs::{VfsBackend, VfsError, VfsStat};

pub struct DevFs {
    stdout_buffer: Mutex<Vec<u8>>,
    stderr_buffer: Mutex<Vec<u8>>,
    stdin_data: Vec<u8>,
}

impl DevFs {
    pub fn new() -> Self {
        Self {
            stdout_buffer: Mutex::new(Vec::new()),
            stderr_buffer: Mutex::new(Vec::new()),
            stdin_data: Vec::new(),
        }
    }

    pub fn set_stdin(&mut self, data: Vec<u8>) {
        self.stdin_data = data;
    }

    pub fn take_stdout(&self) -> Vec<u8> {
        let mut buf = self.stdout_buffer.lock().unwrap();
        std::mem::take(&mut *buf)
    }

    pub fn take_stderr(&self) -> Vec<u8> {
        let mut buf = self.stderr_buffer.lock().unwrap();
        std::mem::take(&mut *buf)
    }
}

impl VfsBackend for DevFs {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        match path {
            "stdin" => Ok(self.stdin_data.clone()),
            "stdout" | "stderr" => Err(VfsError::PermissionDenied),
            _ => Err(VfsError::NotFound),
        }
    }

    fn open_write(&self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        match path {
            "stdout" => {
                let mut buf = self.stdout_buffer.lock().unwrap();
                buf.extend_from_slice(data);
                Ok(())
            }
            "stderr" => {
                let mut buf = self.stderr_buffer.lock().unwrap();
                buf.extend_from_slice(data);
                Ok(())
            }
            "stdin" => Err(VfsError::ReadOnly),
            _ => Err(VfsError::NotFound),
        }
    }

    fn open_append(&self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        self.open_write(path, data)
    }

    fn stat(&self, path: &str) -> Result<VfsStat, VfsError> {
        match path {
            "stdin" | "stdout" | "stderr" => Ok(VfsStat {
                is_file: true,
                is_dir: false,
                size: 0,
                readonly: path == "stdin",
                mtime: 0,
            }),
            "" => Ok(VfsStat {
                is_file: false,
                is_dir: true,
                size: 0,
                readonly: true,
                mtime: 0,
            }),
            _ => Err(VfsError::NotFound),
        }
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, VfsError> {
        if path.is_empty() {
            Ok(vec!["stdin".into(), "stdout".into(), "stderr".into()])
        } else {
            Err(VfsError::NotDirectory)
        }
    }

    fn mkdir(&self, _path: &str) -> Result<(), VfsError> { Err(VfsError::ReadOnly) }
    fn unlink(&self, _path: &str) -> Result<(), VfsError> { Err(VfsError::PermissionDenied) }
    fn rename(&self, _from: &str, _to: &str) -> Result<(), VfsError> { Err(VfsError::PermissionDenied) }

    fn exists(&self, path: &str) -> bool {
        matches!(path, "" | "stdin" | "stdout" | "stderr")
    }

    fn is_readonly(&self) -> bool {
        false // stdout/stderr are writable
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/vfs/dev.rs
git commit -m "feat(vfs): add DevFs pseudo-device filesystem for stdio"
```

---

### Task 6: MoltVfsFile (File Handle Bridge)

**Files:**
- Create: `runtime/molt-runtime/src/vfs/file.rs`

- [ ] **Step 1: Write MoltVfsFile**

Create `runtime/molt-runtime/src/vfs/file.rs`:

```rust
//! File handle wrapper bridging VFS Vec<u8> operations with the runtime's
//! file object expectations (cursor-based read, buffered write, close flush).

use std::sync::Arc;
use crate::vfs::{VfsBackend, VfsError};

pub struct MoltVfsFile {
    content: Vec<u8>,
    cursor: usize,
    path: String,
    writable: bool,
    dirty: bool,
    backend: Arc<dyn VfsBackend>,
}

impl MoltVfsFile {
    pub fn open_read(backend: Arc<dyn VfsBackend>, path: &str) -> Result<Self, VfsError> {
        let content = backend.open_read(path)?;
        Ok(Self {
            content,
            cursor: 0,
            path: path.to_string(),
            writable: false,
            dirty: false,
            backend,
        })
    }

    pub fn open_write(backend: Arc<dyn VfsBackend>, path: &str) -> Result<Self, VfsError> {
        // Start with empty content for write mode
        Ok(Self {
            content: Vec::new(),
            cursor: 0,
            path: path.to_string(),
            writable: true,
            dirty: false,
            backend,
        })
    }

    pub fn open_append(backend: Arc<dyn VfsBackend>, path: &str) -> Result<Self, VfsError> {
        let content = backend.open_read(path).unwrap_or_default();
        let cursor = content.len();
        Ok(Self {
            content,
            cursor,
            path: path.to_string(),
            writable: true,
            dirty: false,
            backend,
        })
    }

    pub fn read(&mut self, n: usize) -> Vec<u8> {
        let end = (self.cursor + n).min(self.content.len());
        let data = self.content[self.cursor..end].to_vec();
        self.cursor = end;
        data
    }

    pub fn read_all(&mut self) -> Vec<u8> {
        let data = self.content[self.cursor..].to_vec();
        self.cursor = self.content.len();
        data
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, VfsError> {
        if !self.writable {
            return Err(VfsError::ReadOnly);
        }
        self.content.extend_from_slice(data);
        self.cursor = self.content.len();
        self.dirty = true;
        Ok(data.len())
    }

    pub fn flush(&mut self) -> Result<(), VfsError> {
        if self.dirty && self.writable {
            self.backend.open_write(&self.path, &self.content)?;
            self.dirty = false;
        }
        Ok(())
    }

    pub fn close(mut self) -> Result<(), VfsError> {
        self.flush()
    }

    pub fn tell(&self) -> usize {
        self.cursor
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/vfs/file.rs
git commit -m "feat(vfs): add MoltVfsFile cursor-based file handle bridge"
```

---

### Task 7: Rust Unit Tests

**Files:**
- Add tests to: `runtime/molt-runtime/src/vfs/mod.rs` (or as `#[cfg(test)]` blocks in each file)

- [ ] **Step 1: Add unit tests for path normalization and MountTable**

Add to the bottom of `runtime/molt-runtime/src/vfs/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::bundle::BundleFs;
    use crate::vfs::tmp::TmpFs;

    #[test]
    fn test_normalize_rejects_empty() {
        assert!(normalize_path("").is_none());
    }

    #[test]
    fn test_normalize_rejects_relative() {
        assert!(normalize_path("foo/bar").is_none());
    }

    #[test]
    fn test_normalize_rejects_traversal() {
        assert!(normalize_path("/bundle/../etc/passwd").is_none());
    }

    #[test]
    fn test_normalize_rejects_bare_root() {
        assert!(normalize_path("/").is_none());
    }

    #[test]
    fn test_normalize_collapses_slashes() {
        assert_eq!(normalize_path("/bundle//file.txt"), Some("/bundle/file.txt".into()));
    }

    #[test]
    fn test_normalize_resolves_dot() {
        assert_eq!(normalize_path("/bundle/./file.txt"), Some("/bundle/file.txt".into()));
    }

    #[test]
    fn test_mount_table_resolves_longest_prefix() {
        let mut table = MountTable::new();
        let bundle = Arc::new(BundleFs::from_entries(vec![
            ("file.txt".into(), b"hello".to_vec()),
        ]));
        let tmp = Arc::new(TmpFs::new(64));
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
    fn test_mount_table_no_match() {
        let table = MountTable::new();
        assert!(table.resolve("/unknown/path").is_none());
    }

    #[test]
    fn test_bundle_fs_read() {
        let fs = BundleFs::from_entries(vec![
            ("hello.txt".into(), b"world".to_vec()),
        ]);
        assert_eq!(fs.open_read("hello.txt").unwrap(), b"world");
        assert!(fs.open_read("missing.txt").is_err());
    }

    #[test]
    fn test_bundle_fs_readonly() {
        let fs = BundleFs::from_entries(vec![]);
        assert!(fs.open_write("file.txt", b"data").is_err());
        assert!(fs.is_readonly());
    }

    #[test]
    fn test_bundle_fs_readdir() {
        let fs = BundleFs::from_entries(vec![
            ("a.txt".into(), b"".to_vec()),
            ("sub/b.txt".into(), b"".to_vec()),
        ]);
        let mut entries = fs.readdir("").unwrap();
        entries.sort();
        assert_eq!(entries, vec!["a.txt", "sub"]);
    }

    #[test]
    fn test_tmp_fs_write_read() {
        let fs = TmpFs::new(64);
        fs.open_write("file.txt", b"hello").unwrap();
        assert_eq!(fs.open_read("file.txt").unwrap(), b"hello");
    }

    #[test]
    fn test_tmp_fs_quota() {
        let fs = TmpFs::new(0); // 0 MB quota
        assert!(matches!(
            fs.open_write("file.txt", b"data"),
            Err(VfsError::QuotaExceeded)
        ));
    }

    #[test]
    fn test_tmp_fs_rename() {
        let fs = TmpFs::new(64);
        fs.open_write("a.txt", b"content").unwrap();
        fs.rename("a.txt", "b.txt").unwrap();
        assert!(!fs.exists("a.txt"));
        assert_eq!(fs.open_read("b.txt").unwrap(), b"content");
    }

    #[test]
    fn test_tmp_fs_unlink() {
        let fs = TmpFs::new(64);
        fs.open_write("file.txt", b"data").unwrap();
        fs.unlink("file.txt").unwrap();
        assert!(!fs.exists("file.txt"));
    }

    #[test]
    fn test_tmp_fs_mkdir() {
        let fs = TmpFs::new(64);
        fs.mkdir("subdir").unwrap();
        assert!(fs.stat("subdir").unwrap().is_dir);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p molt-runtime --lib vfs 2>&1 | tail -15`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/vfs/
git commit -m "test(vfs): add unit tests for path normalization, MountTable, BundleFs, TmpFs"
```

---

### Task 8: Integration with io.rs (open() routing)

**Files:**
- Modify: `runtime/molt-runtime/src/builtins/io.rs:3678`

- [ ] **Step 1: Add VFS dispatch to open_impl**

In `runtime/molt-runtime/src/builtins/io.rs`, find the line `file = match mode_info.options.open(&path)` (~line 3678). Add VFS dispatch before it:

```rust
// VFS dispatch: route through virtual filesystem on WASM targets
#[cfg(target_arch = "wasm32")]
if let Some(vfs_state) = runtime_state(_py).vfs.as_ref() {
    if let Some((mount_prefix, backend, rel_path)) = vfs_state.resolve(&path_str) {
        // Check capability
        let is_write = mode_info.writable;
        if let Err(vfs_err) = crate::vfs::caps::check_mount_capability(
            &mount_prefix,
            is_write,
            &|cap| has_capability(_py, cap),
        ) {
            return raise_exception::<_>(_py, "PermissionError", &vfs_err.to_string());
        }
        // Open through VFS
        if mode_info.readable && !mode_info.writable {
            match backend.open_read(&rel_path) {
                Ok(data) => {
                    // Create a MoltVfsFile and wrap in runtime file object
                    // (integration with runtime file handle machinery)
                    // For now: return the data as a bytes-backed TextIOWrapper or BytesIO
                    todo!("VFS file handle integration — Task for Layer 2");
                }
                Err(vfs_err) => {
                    return raise_exception::<_>(_py, "FileNotFoundError", &vfs_err.to_string());
                }
            }
        }
    }
}
// Fallback to std::fs for native builds or non-VFS paths
file = match mode_info.options.open(&path) {
```

Note: The full file handle integration requires the host adapter (Layer 2) to inject the VFS state into the runtime. This task establishes the dispatch point; the actual file wrapping is completed in Plan B.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/builtins/io.rs
git commit -m "feat(vfs): add VFS dispatch point in open_impl for WASM targets"
```

---

## Execution Notes

**Dependencies:**
- Tasks 1-6 are independent and can be executed in parallel (they create separate files)
- Task 7 depends on Tasks 1-5 (tests import the implementations)
- Task 8 depends on Task 1 (uses VfsState)

**What this plan does NOT include (deferred to Plans B/C/D):**
- Host adapter setup (populating VFS at instantiation) — Plan B
- Bundle packaging (`--bundle` CLI flag) — Plan C
- Snapshot generation/restore — Plan D
- Module import integration (modules.rs changes) — Plan B (needs host to inject VFS)
- Browser/Cloudflare JS host code — Plan B

**Testing approach:**
- Unit tests in Rust cover the VFS core (Task 7)
- Integration tests in Python require the host adapter (Plan B) to populate the VFS
- E2E tests require packaging (Plan C)
