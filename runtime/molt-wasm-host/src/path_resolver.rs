use super::*;
use molt_wasm_host::sha256_digest_hex;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
struct RuntimeManifest {
    version: u32,
    mode: String,
    modules: RuntimeManifestModules,
}

#[derive(Debug, Deserialize)]
struct RuntimeManifestModules {
    app: Option<RuntimeManifestModule>,
    runtime: Option<RuntimeManifestModule>,
    linked: Option<RuntimeManifestModule>,
}

#[derive(Debug, Deserialize)]
struct RuntimeManifestModule {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug)]
pub(super) struct ResolvedExecutionModules {
    pub(super) manifest_path: PathBuf,
    pub(super) main: ModuleSource,
    pub(super) runtime: Option<ModuleSource>,
    pub(super) linked: bool,
}

/// One owned byte sequence for validation, fact scanning and compilation.
/// Reopening the path after manifest admission would discard that admission.
#[derive(Debug)]
pub(super) struct ModuleSource {
    path: PathBuf,
    bytes: Vec<u8>,
    sha256: OnceLock<[u8; 32]>,
}

impl ModuleSource {
    pub(super) fn read(path: PathBuf, label: &str, expected_size: Option<u64>) -> Result<Self> {
        let path = std::path::absolute(path).context("resolve module input path")?;
        let started = Instant::now();
        // Reject devices/pipes before open. On POSIX, nonblocking open also
        // prevents a concurrent regular-file-to-FIFO substitution from hanging.
        if !fs::metadata(&path)
            .with_context(|| format!("inspect {label} module {}", path.display()))?
            .is_file()
        {
            bail!("{label} module is not a file: {}", path.display());
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options
            .open(&path)
            .with_context(|| format!("open {label} module {}", path.display()))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("{label} module is not a file: {}", path.display());
        }
        if let Some(expected) = expected_size
            && metadata.len() != expected
        {
            bail!(
                "{label} size mismatch: manifest={expected} actual={}",
                metadata.len()
            );
        }
        let expected = expected_size.unwrap_or(metadata.len());
        let bound = expected
            .checked_add(1)
            .context("module size exceeds read bound")?;
        let mut bytes = Vec::new();
        let size = usize::try_from(bound).context("module size exceeds host address space")?;
        bytes
            .try_reserve_exact(size)
            .context("allocate module source buffer")?;
        // Read at most the admitted size plus one sentinel byte. A concurrently
        // growing file cannot turn a bounded artifact read into an endless one.
        file.take(bound)
            .read_to_end(&mut bytes)
            .with_context(|| format!("read {label} module {}", path.display()))?;
        if bytes.len() as u64 != expected {
            bail!(
                "{label} size changed while reading: expected={expected} actual={}",
                bytes.len()
            );
        }
        log::debug!("read {label} module in {:?}", started.elapsed());
        Ok(Self {
            path,
            bytes,
            sha256: OnceLock::new(),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// One lazy identity of the admitted bytes, independent of later path changes.
    pub(super) fn sha256(&self) -> &[u8; 32] {
        self.sha256
            .get_or_init(|| Sha256::digest(&self.bytes).into())
    }
}

#[derive(Debug)]
pub(super) enum ExecutionRequest {
    MoltApplication { manifest: Option<String> },
    WasiCommand { module: String },
}

#[derive(Debug)]
pub(super) enum ResolvedExecution {
    MoltApplication(ResolvedExecutionModules),
    WasiCommand { module: ModuleSource },
}

pub(super) fn select_manifest_path(
    arg: Option<PathBuf>,
    env_path: Option<PathBuf>,
    cwd: &Path,
) -> PathBuf {
    arg.or(env_path)
        .unwrap_or_else(|| cwd.join("dist").join("manifest.json"))
}

fn resolve_manifest_module(
    manifest_path: &Path,
    descriptor: Option<&RuntimeManifestModule>,
    label: &str,
) -> Result<ModuleSource> {
    let descriptor =
        descriptor.with_context(|| format!("runtime manifest missing modules.{label}"))?;
    if descriptor.path.is_empty() {
        bail!("runtime manifest modules.{label}.path is empty");
    }
    // Manifest assets are portable adjacent file names, not host-specific
    // absolute paths, traversal components or Windows drive/stream selectors.
    if descriptor.path.contains(['/', '\\', ':'])
        || Path::new(&descriptor.path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(descriptor.path.as_str())
    {
        bail!("runtime manifest modules.{label}.path must name an adjacent file");
    }
    if descriptor.sha256.len() != 64
        || !descriptor
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("runtime manifest modules.{label}.sha256 is invalid");
    }
    let manifest_dir = manifest_path
        .parent()
        .context("runtime manifest path has no parent directory")?;
    let module_path = manifest_dir.join(&descriptor.path);
    let source = ModuleSource::read(module_path, label, Some(descriptor.size))?;
    let actual = sha256_digest_hex(source.sha256());
    if actual != descriptor.sha256 {
        bail!(
            "{label} SHA-256 mismatch: manifest={} actual={actual}",
            descriptor.sha256
        );
    }
    Ok(source)
}

pub(super) fn resolve_execution_modules(arg: Option<String>) -> Result<ResolvedExecutionModules> {
    let cwd = env::current_dir().context("failed to resolve current directory")?;
    let env_path = env::var_os("MOLT_WASM_MANIFEST_PATH").map(PathBuf::from);
    let manifest_path =
        std::path::absolute(select_manifest_path(arg.map(PathBuf::from), env_path, &cwd))
            .context("resolve runtime manifest path")?;
    if manifest_path.extension().and_then(|value| value.to_str()) != Some("json") {
        bail!(
            "molt-wasm-host accepts a runtime manifest path, not a module path; pass manifest.json"
        );
    }
    let manifest_bytes = fs::read(&manifest_path).with_context(|| {
        format!(
            "failed to read runtime manifest: {}",
            manifest_path.display()
        )
    })?;
    let manifest: RuntimeManifest = serde_json::from_slice(&manifest_bytes).with_context(|| {
        format!(
            "failed to decode runtime manifest: {}",
            manifest_path.display()
        )
    })?;
    if manifest.version != 2 {
        bail!("runtime manifest must use version 2");
    }
    let (main, runtime, linked) = match manifest.mode.as_str() {
        "linked" => (
            resolve_manifest_module(&manifest_path, manifest.modules.linked.as_ref(), "linked")?,
            None,
            true,
        ),
        "split-runtime" => (
            resolve_manifest_module(&manifest_path, manifest.modules.app.as_ref(), "app")?,
            Some(resolve_manifest_module(
                &manifest_path,
                manifest.modules.runtime.as_ref(),
                "runtime",
            )?),
            false,
        ),
        mode => bail!("runtime manifest has unsupported mode: {mode}"),
    };
    Ok(ResolvedExecutionModules {
        manifest_path,
        main,
        runtime,
        linked,
    })
}

pub(super) fn resolve_execution(request: ExecutionRequest) -> Result<ResolvedExecution> {
    match request {
        ExecutionRequest::MoltApplication { manifest } => Ok(ResolvedExecution::MoltApplication(
            resolve_execution_modules(manifest)?,
        )),
        ExecutionRequest::WasiCommand { module } => {
            if module.is_empty() {
                bail!("--wasi-command module path is empty");
            }
            Ok(ResolvedExecution::WasiCommand {
                module: ModuleSource::read(PathBuf::from(module), "WASI command", None)?,
            })
        }
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_env = env::var("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_exports_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("MOLT_WASM_DB_EXPORTS").or_else(|_| env::var("MOLT_WORKER_EXPORTS"))
    {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    let packaged = PathBuf::from("src/molt_accel/default_exports.json");
    if packaged.exists() {
        return Some(packaged);
    }
    let demo = PathBuf::from("demo/molt_worker_app/molt_exports.json");
    if demo.exists() {
        return Some(demo);
    }
    None
}

pub(super) fn resolve_worker_cmd() -> Result<Vec<String>> {
    if let Ok(cmd) = env::var("MOLT_WASM_DB_WORKER_CMD").or_else(|_| env::var("MOLT_WORKER_CMD")) {
        let parts = cmd
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if parts.is_empty() {
            bail!("MOLT_WASM_DB_WORKER_CMD is empty");
        }
        return Ok(parts);
    }
    let worker = find_in_path("molt-worker").or_else(|| find_in_path("molt_worker"));
    let Some(worker) = worker else {
        bail!("molt-worker not found; set MOLT_WASM_DB_WORKER_CMD or MOLT_WORKER_CMD");
    };
    let exports_path = resolve_exports_path()
        .context("molt-worker exports manifest not found (set MOLT_WASM_DB_EXPORTS)")?;
    let mut cmd = vec![
        worker.to_string_lossy().to_string(),
        "--stdio".into(),
        "--exports".into(),
    ];
    cmd.push(exports_path.to_string_lossy().to_string());
    if let Ok(compiled) = env::var("MOLT_WASM_DB_COMPILED_EXPORTS") {
        cmd.push("--compiled-exports".into());
        cmd.push(compiled);
    }
    Ok(cmd)
}

pub(super) fn resolve_timeout_ms() -> u64 {
    if let Ok(raw) =
        env::var("MOLT_WASM_DB_TIMEOUT_MS").or_else(|_| env::var("MOLT_DB_QUERY_TIMEOUT_MS"))
        && let Ok(val) = raw.parse::<u64>()
    {
        return val;
    }
    250
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_source_digest_is_lazy_and_uses_owned_bytes() {
        let source = ModuleSource {
            path: PathBuf::from("unopened-module-source.wasm"),
            bytes: b"abc".to_vec(),
            sha256: OnceLock::new(),
        };
        assert!(source.sha256.get().is_none());
        let digest = source.sha256();
        assert_eq!(
            sha256_digest_hex(digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(std::ptr::eq(digest, source.sha256.get().unwrap()));
        assert!(std::ptr::eq(digest, source.sha256()));
        assert_eq!(source.bytes(), b"abc");
    }
}
