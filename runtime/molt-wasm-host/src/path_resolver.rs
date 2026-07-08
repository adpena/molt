use super::*;

pub(super) fn resolve_wasm_path(arg: Option<String>) -> Result<PathBuf> {
    let env_path = env::var("MOLT_WASM_PATH").ok().map(PathBuf::from);
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = wasm_path_candidates(arg.map(PathBuf::from), env_path, &cwd);

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("WASM path not found (arg, MOLT_WASM_PATH, or ./dist/output.wasm)");
}

pub(super) fn resolve_linked_path(wasm_path: &Path) -> Option<PathBuf> {
    let env_path = env::var("MOLT_WASM_LINKED_PATH").ok().map(PathBuf::from);
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    linked_path_candidates(wasm_path, env_path, &cwd)
        .into_iter()
        .find(|candidate| candidate.exists())
}

pub(super) fn wasm_path_candidates(
    arg: Option<PathBuf>,
    env_path: Option<PathBuf>,
    cwd: &Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = arg {
        candidates.push(path);
    }
    if let Some(path) = env_path
        && !candidates.iter().any(|candidate| candidate == &path)
    {
        candidates.push(path);
    }
    let canonical = cwd.join("dist").join("output.wasm");
    if !candidates.iter().any(|candidate| candidate == &canonical) {
        candidates.push(canonical);
    }
    candidates
}

pub(super) fn linked_path_candidates(
    wasm_path: &Path,
    env_path: Option<PathBuf>,
    cwd: &Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env_path {
        candidates.push(path);
    }
    if let Some(stem) = wasm_path.file_stem().and_then(|s| s.to_str()) {
        let ext = wasm_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("wasm");
        let sibling = wasm_path.with_file_name(format!("{stem}_linked.{ext}"));
        if !candidates.iter().any(|candidate| candidate == &sibling) {
            candidates.push(sibling);
        }
    }
    let canonical = cwd.join("dist").join("output_linked.wasm");
    if !candidates.iter().any(|candidate| candidate == &canonical) {
        candidates.push(canonical);
    }
    candidates
}

pub(super) fn prefer_linked() -> bool {
    match env::var("MOLT_WASM_PREFER_LINKED") {
        Ok(val) => !matches!(val.to_lowercase().as_str(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

pub(super) fn force_linked() -> bool {
    matches!(env::var("MOLT_WASM_LINKED").as_deref(), Ok("1"))
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
