use super::*;

#[derive(Debug, Deserialize)]
struct RuntimeManifest {
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
    pub(super) main_path: PathBuf,
    pub(super) runtime_path: Option<PathBuf>,
    pub(super) linked: bool,
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
) -> Result<PathBuf> {
    let descriptor =
        descriptor.with_context(|| format!("runtime manifest missing modules.{label}"))?;
    if descriptor.path.is_empty() {
        bail!("runtime manifest modules.{label}.path is empty");
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
    let metadata = fs::metadata(&module_path).with_context(|| {
        format!(
            "runtime manifest modules.{label}.path is unreadable: {}",
            module_path.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "runtime manifest modules.{label}.path is not a file: {}",
            module_path.display()
        );
    }
    if metadata.len() != descriptor.size {
        bail!(
            "{label} size mismatch: manifest={} actual={}",
            descriptor.size,
            metadata.len()
        );
    }
    let mut file = fs::File::open(&module_path)
        .with_context(|| format!("failed to open {label}: {}", module_path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to hash {label}: {}", module_path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != descriptor.sha256 {
        bail!(
            "{label} SHA-256 mismatch: manifest={} actual={actual}",
            descriptor.sha256
        );
    }
    Ok(module_path)
}

pub(super) fn resolve_execution_modules(arg: Option<String>) -> Result<ResolvedExecutionModules> {
    let cwd = env::current_dir().context("failed to resolve current directory")?;
    let env_path = env::var_os("MOLT_WASM_MANIFEST_PATH").map(PathBuf::from);
    let manifest_path = select_manifest_path(arg.map(PathBuf::from), env_path, &cwd);
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
    let (main_path, runtime_path, linked) = match manifest.mode.as_str() {
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
        main_path,
        runtime_path,
        linked,
    })
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
