use std::io;
use std::path::Path;

#[cfg(any(feature = "native-backend", molt_backend_daemon))]
use molt_artifact_publish::write_bytes_atomically;

pub(crate) fn ensure_output_parent_dir(output_file: &str) -> io::Result<()> {
    let path = Path::new(output_file);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(molt_backend_daemon)]
pub(crate) fn write_cached_output(
    path: &str,
    bytes: &[u8],
    skip_if_synced: bool,
) -> io::Result<bool> {
    if skip_if_synced {
        return Ok(false);
    }
    write_output(path, bytes)?;
    Ok(true)
}

#[cfg(molt_backend_daemon)]
pub(crate) fn write_output(path: &str, bytes: &[u8]) -> io::Result<()> {
    write_output_path(Path::new(path), bytes)
}

#[cfg(any(feature = "native-backend", molt_backend_daemon))]
pub(crate) fn write_output_path(output_path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_bytes_atomically(output_path, bytes)
}
