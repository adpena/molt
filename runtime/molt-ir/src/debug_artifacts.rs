//! Backend debug artifacts and scratch objects live under one directory that
//! the caller names in `MOLT_DEBUG_ARTIFACT_DIR`. The molt CLI resolves it
//! through `molt.dx.scratch_dir` and always passes it (HF-111). The backend
//! never derives a Molt root, a checkout path or a working-directory path
//! itself; a backend started without the variable (a Rust test, a manual run)
//! uses the platform temp directory.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The one input that places backend debug artifacts.
pub const DEBUG_ARTIFACT_DIR_ENV: &str = "MOLT_DEBUG_ARTIFACT_DIR";
const DEFAULT_DIRNAME: &str = "molt-backend";

static UNIQUE_ARTIFACT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The debug artifact root for a `MOLT_DEBUG_ARTIFACT_DIR` value: the value
/// itself, or `<platform temp dir>/molt-backend` when it is unset or empty.
pub fn debug_artifact_root(explicit: Option<&OsStr>) -> PathBuf {
    match explicit {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::temp_dir().join(DEFAULT_DIRNAME),
    }
}

fn configured_debug_artifact_root() -> PathBuf {
    debug_artifact_root(std::env::var_os(DEBUG_ARTIFACT_DIR_ENV).as_deref())
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn prepare_under(root: &Path, relative_path: &Path) -> io::Result<PathBuf> {
    let path = root.join(relative_path);
    ensure_parent(&path)?;
    Ok(path)
}

/// A fresh sibling of `base` named `<stem>.<pid>.<nonce>.tmp[.<ext>]`.
fn unique_sibling(base: &Path) -> PathBuf {
    let unique = UNIQUE_ARTIFACT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("artifact");
    let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("");
    let nonce = nanos ^ (unique as u128);
    let file_name = if ext.is_empty() {
        format!("{stem}.{}.{nonce}.tmp", std::process::id())
    } else {
        format!("{stem}.{}.{nonce}.tmp.{ext}", std::process::id())
    };
    base.with_file_name(file_name)
}

pub fn prepare_debug_artifact_path(relative_path: impl AsRef<Path>) -> io::Result<PathBuf> {
    prepare_under(&configured_debug_artifact_root(), relative_path.as_ref())
}

pub fn prepare_unique_debug_artifact_path(relative_path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let base = prepare_under(&configured_debug_artifact_root(), relative_path.as_ref())?;
    Ok(unique_sibling(&base))
}

pub fn write_debug_artifact(
    relative_path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
) -> io::Result<PathBuf> {
    let path = prepare_debug_artifact_path(relative_path)?;
    fs::write(&path, bytes)?;
    Ok(path)
}

pub fn append_debug_artifact(
    relative_path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
) -> io::Result<PathBuf> {
    let path = prepare_debug_artifact_path(relative_path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(bytes.as_ref())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScratchRoot(PathBuf);

    impl ScratchRoot {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "molt-ir-debug-artifacts-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_directory_is_the_root() {
        let explicit = Path::new("/explicit/backend-debug");
        assert_eq!(
            debug_artifact_root(Some(explicit.as_os_str())),
            explicit.to_path_buf()
        );
    }

    #[test]
    fn unset_or_empty_directory_uses_the_platform_temp_directory() {
        let fallback = std::env::temp_dir().join("molt-backend");
        assert_eq!(debug_artifact_root(None), fallback);
        assert_eq!(debug_artifact_root(Some(OsStr::new(""))), fallback);
    }

    #[test]
    fn prepared_paths_create_their_parent_under_the_root() {
        let root = ScratchRoot::new("prepare");
        let path = prepare_under(&root.0, Path::new("tir/roundtrip/example.txt")).unwrap();
        assert_eq!(
            path,
            root.0.join("tir").join("roundtrip").join("example.txt")
        );
        assert!(root.0.join("tir").join("roundtrip").is_dir());
        assert!(!path.exists());
    }

    #[test]
    fn unique_paths_are_distinct_siblings_of_the_base() {
        let root = ScratchRoot::new("unique");
        let base = prepare_under(&root.0, Path::new("llvm/output.o")).unwrap();
        let a = unique_sibling(&base);
        let b = unique_sibling(&base);
        assert_ne!(a, b);
        for path in [&a, &b] {
            assert_eq!(path.parent(), Some(root.0.join("llvm").as_path()));
            let name = path.file_name().unwrap().to_string_lossy();
            assert!(name.starts_with("output."), "{name}");
            assert!(name.ends_with(".tmp.o"), "{name}");
        }
    }
}
