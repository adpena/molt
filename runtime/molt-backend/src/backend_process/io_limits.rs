use super::*;

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

#[cfg(feature = "native-backend")]
pub(crate) fn write_json_artifact<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    ensure_output_parent_dir(path.to_str().unwrap_or_default())?;
    let file = File::create(path)?;
    let writer = io::BufWriter::new(file);
    serde_json::to_writer(writer, value).map_err(io::Error::other)
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_json_artifact<T: serde::de::DeserializeOwned>(
    path: &Path,
    label: &str,
) -> io::Result<T> {
    let file = File::open(path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to open {label} '{}': {err}", path.display()),
        )
    })?;
    let reader = io::BufReader::new(file);
    serde_json::from_reader(reader).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {label} '{}': {err}", path.display()),
        )
    })
}

pub(crate) fn env_usize_limit(name: &str, default: usize, min_value: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value >= min_value)
        .unwrap_or(default)
}

#[cfg(any(unix, test))]
pub(crate) fn daemon_request_limit_bytes() -> usize {
    env_usize_limit(
        "MOLT_BACKEND_DAEMON_REQUEST_LIMIT_BYTES",
        DEFAULT_DAEMON_REQUEST_LIMIT_BYTES,
        1024,
    )
}

pub(crate) fn stdin_request_limit_bytes() -> usize {
    env_usize_limit(
        "MOLT_BACKEND_STDIN_REQUEST_LIMIT_BYTES",
        DEFAULT_STDIN_REQUEST_LIMIT_BYTES,
        1024,
    )
}

#[cfg(any(unix, test))]
pub(crate) fn daemon_max_jobs() -> usize {
    env_usize_limit("MOLT_BACKEND_DAEMON_MAX_JOBS", DEFAULT_DAEMON_MAX_JOBS, 1)
}

#[derive(Debug)]
pub(crate) struct RequestBoundedRead<R> {
    pub(crate) inner: R,
    pub(crate) remaining: usize,
    pub(crate) limit_bytes: usize,
    pub(crate) context: &'static str,
}

impl<R: Read> RequestBoundedRead<R> {
    pub(crate) fn new(inner: R, limit_bytes: usize, context: &'static str) -> Self {
        Self {
            inner,
            remaining: limit_bytes,
            limit_bytes,
            context,
        }
    }
}

impl<R: Read> Read for RequestBoundedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.inner.read(&mut probe) {
                Ok(0) => Ok(0),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} exceeded {} byte limit", self.context, self.limit_bytes),
                )),
                Err(err) => Err(err),
            };
        }

        let read_len = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..read_len])?;
        self.remaining = self.remaining.saturating_sub(n);
        Ok(n)
    }
}

pub(crate) fn read_bounded_request_bytes<R: Read>(
    reader: R,
    limit_bytes: usize,
    context: &'static str,
) -> io::Result<Vec<u8>> {
    let mut bounded = RequestBoundedRead::new(reader, limit_bytes, context);
    let mut bytes = Vec::new();
    bounded.read_to_end(&mut bytes)?;
    Ok(bytes)
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
