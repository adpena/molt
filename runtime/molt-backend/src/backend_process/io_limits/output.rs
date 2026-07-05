use std::fs::File;
use std::io;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BackendOutputKind {
    Luau,
    Rust,
    Wasm,
    Native,
}

pub(crate) fn ensure_output_parent_dir(output_file: &str) -> io::Result<()> {
    let path = Path::new(output_file);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg_attr(
    not(any(
        feature = "luau-backend",
        feature = "rust-backend",
        feature = "wasm-backend"
    )),
    allow(dead_code)
)]
pub(crate) fn create_backend_output_file(output_file: &str) -> io::Result<File> {
    ensure_output_parent_dir(output_file)?;
    match File::create(output_file) {
        Ok(file) => Ok(file),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            // Shared cache/build roots may be pruned between early setup and
            // final artifact emission. Recreate the parent at the point of
            // use and retry once so output emission is authoritative.
            ensure_output_parent_dir(output_file)?;
            File::create(output_file)
        }
        Err(err) => Err(err),
    }
}

pub(crate) fn default_backend_output_path(kind: BackendOutputKind) -> &'static str {
    match kind {
        BackendOutputKind::Luau => "dist/output.luau",
        BackendOutputKind::Rust => "dist/output.rs",
        BackendOutputKind::Wasm => "dist/output.wasm",
        BackendOutputKind::Native => "dist/output.o",
    }
}

pub(crate) fn resolve_backend_output_path(
    output_path: Option<&str>,
    kind: BackendOutputKind,
) -> &str {
    output_path.unwrap_or(default_backend_output_path(kind))
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
#[cfg(any(unix, test))]
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

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
#[cfg(any(unix, test))]
pub(crate) fn write_output(path: &str, bytes: &[u8]) -> io::Result<()> {
    write_output_path(Path::new(path), bytes)
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn write_output_path(output_path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let base_name = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_name = format!(".{base_name}.{}.{}.tmp", std::process::id(), nonce);
    let tmp_path = output_path.with_file_name(tmp_name);
    let mut file = File::create(&tmp_path)?;
    file.write_all(bytes)?;
    drop(file);

    match std::fs::rename(&tmp_path, output_path) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            let _ = std::fs::remove_file(output_path);
            match std::fs::rename(&tmp_path, output_path) {
                Ok(()) => Ok(()),
                Err(second_err) => {
                    let _ = std::fs::remove_file(&tmp_path);
                    Err(io::Error::new(
                        second_err.kind(),
                        format!(
                            "failed to atomically replace output (first: {first_err}; second: {second_err})"
                        ),
                    ))
                }
            }
        }
    }
}
