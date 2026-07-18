use std::io;
use std::path::Path;

#[cfg(any(feature = "native-backend", all(unix, feature = "wasm-backend"), test))]
use crate::backend_process::atomic_publish::write_bytes_atomically;

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
#[cfg(any(feature = "native-backend", all(unix, feature = "wasm-backend"), test))]
pub(crate) fn write_output_path(output_path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_bytes_atomically(output_path, bytes)
}
