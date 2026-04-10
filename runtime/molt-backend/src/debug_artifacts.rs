use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE_ARTIFACT_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn repo_debug_artifact_root(repo_root: &Path) -> PathBuf {
    repo_root.join("tmp").join("molt-backend")
}

fn default_debug_artifact_root() -> PathBuf {
    if let Some(explicit) = std::env::var_os("MOLT_DEBUG_ARTIFACT_DIR") {
        return PathBuf::from(explicit);
    }
    if let Some(repo_root) = std::env::var_os("MOLT_EXT_ROOT") {
        return repo_debug_artifact_root(Path::new(&repo_root));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    repo_debug_artifact_root(&cwd)
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn prepare_debug_artifact_path(relative_path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = default_debug_artifact_root().join(relative_path.as_ref());
    ensure_parent(&path)?;
    Ok(path)
}

pub fn prepare_unique_debug_artifact_path(relative_path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let base = default_debug_artifact_root().join(relative_path.as_ref());
    ensure_parent(&base)?;
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
    let file_name = if ext.is_empty() {
        format!("{stem}.{}.{}.tmp", std::process::id(), nanos ^ (unique as u128))
    } else {
        format!(
            "{stem}.{}.{}.tmp.{ext}",
            std::process::id(),
            nanos ^ (unique as u128)
        )
    };
    Ok(base.with_file_name(file_name))
}

pub fn write_debug_artifact_under(
    repo_root: &Path,
    relative_path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
) -> io::Result<PathBuf> {
    let path = repo_debug_artifact_root(repo_root).join(relative_path.as_ref());
    ensure_parent(&path)?;
    fs::write(&path, bytes)?;
    Ok(path)
}

pub fn write_debug_artifact(
    relative_path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
) -> io::Result<PathBuf> {
    let path = default_debug_artifact_root().join(relative_path.as_ref());
    ensure_parent(&path)?;
    fs::write(&path, bytes)?;
    Ok(path)
}

pub fn append_debug_artifact(
    relative_path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
) -> io::Result<PathBuf> {
    let path = default_debug_artifact_root().join(relative_path.as_ref());
    ensure_parent(&path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(bytes.as_ref())?;
    Ok(path)
}
