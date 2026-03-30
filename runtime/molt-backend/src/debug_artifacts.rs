use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

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
