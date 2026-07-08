use super::*;

fn precompiled_enabled() -> bool {
    matches!(env::var("MOLT_WASM_PRECOMPILED").as_deref(), Ok("1"))
}

fn precompiled_write_enabled() -> bool {
    matches!(env::var("MOLT_WASM_PRECOMPILED_WRITE").as_deref(), Ok("1"))
}

fn resolve_precompiled_path(wasm_path: &Path, override_env: &str) -> Option<PathBuf> {
    if !precompiled_enabled() {
        return None;
    }
    if let Ok(path) = env::var(override_env)
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    Some(wasm_path.with_extension("cwasm"))
}

pub(super) fn load_or_compile_module(
    engine: &Engine,
    wasm_path: &Path,
    label: &str,
    override_env: &str,
) -> Result<Module> {
    if let Some(precompiled) = resolve_precompiled_path(wasm_path, override_env)
        && precompiled.exists()
    {
        debug_log(|| format!("loading {label} precompiled: {precompiled:?}"));
        match unsafe { Module::deserialize_file(engine, &precompiled) } {
            Ok(module) => return Ok(module),
            Err(err) => {
                debug_log(|| format!("precompiled load failed ({label}): {err}"));
            }
        }
    }
    let read_start = Instant::now();
    let wasm_bytes = fs::read(wasm_path).with_context(|| format!("read {label} {wasm_path:?}"))?;
    debug_log(|| format!("read {label} wasm in {:?}", read_start.elapsed()));
    let compile_start = Instant::now();
    let module = Module::new(engine, wasm_bytes)
        .map_err(|err| err.context(format!("compile {label} {wasm_path:?}")))?;
    debug_log(|| format!("compiled {label} module in {:?}", compile_start.elapsed()));
    if precompiled_write_enabled()
        && let Some(precompiled) = resolve_precompiled_path(wasm_path, override_env)
    {
        match module.serialize() {
            Ok(bytes) => {
                let _ = fs::write(&precompiled, bytes);
                debug_log(|| format!("wrote {label} precompiled: {precompiled:?}"));
            }
            Err(err) => {
                debug_log(|| format!("serialize {label} failed: {err}"));
            }
        }
    }
    Ok(module)
}

pub(super) fn build_engine() -> Result<Engine> {
    let mut config = Config::new();
    let cache_toggle = env::var("MOLT_WASM_CACHE").ok();
    let max_stack = env::var("MOLT_WASM_MAX_STACK")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .filter(|val| *val > 0)
        .unwrap_or(8 * 1024 * 1024);
    // Wasmtime 43+ requires async_stack_size >= max_wasm_stack unconditionally.
    // Bump async_stack_size to accommodate, adding headroom for host-side frames.
    config.async_stack_size(max_stack + (128 * 1024));
    config.max_wasm_stack(max_stack);
    debug_log(|| format!("wasmtime max_wasm_stack set to {max_stack}"));
    if cache_toggle.as_deref() != Some("0") {
        let cache_path = env::var("MOLT_WASM_CACHE_CONFIG").ok();
        if cache_toggle.as_deref() == Some("1") || cache_path.is_some() {
            let cache = match cache_path.as_deref() {
                Some(path) => {
                    debug_log(|| format!("wasmtime cache config: {path}"));
                    Cache::from_file(Some(Path::new(path)))?
                }
                None => {
                    debug_log(|| "wasmtime cache config: default".to_string());
                    Cache::from_file(None)?
                }
            };
            config.cache(Some(cache));
            debug_log(|| "wasmtime cache enabled".to_string());
        }
    }
    if matches!(env::var("MOLT_WASM_COMPILE_SERIAL").as_deref(), Ok("1")) {
        config.parallel_compilation(false);
        debug_log(|| "wasmtime parallel compilation disabled".to_string());
    }
    if matches!(env::var("MOLT_WASM_COMPILE_FAST").as_deref(), Ok("1")) {
        config.cranelift_opt_level(OptLevel::None);
        debug_log(|| "wasmtime opt level set to none".to_string());
    }
    // Deterministic mode: canonicalize NaN payloads and disable parallel compilation
    // to ensure reproducible WASM execution across runs and hosts.
    if matches!(env::var("MOLT_DETERMINISTIC").as_deref(), Ok("1")) {
        config.cranelift_nan_canonicalization(true);
        config.parallel_compilation(false);
        debug_log(|| {
            "deterministic mode: NaN canonicalization and serial compilation enabled".to_string()
        });
    }
    config.wasm_function_references(true);
    config.wasm_gc(true);
    Ok(Engine::new(&config)?)
}
