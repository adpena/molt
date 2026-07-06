use super::*;

#[cfg(all(
    any(unix, test),
    any(feature = "native-backend", feature = "wasm-backend")
))]
pub(crate) fn daemon_memory_cache_allowed_for_job(job: &DaemonJobRequest) -> bool {
    if job.is_wasm {
        return true;
    }
    #[cfg(feature = "native-backend")]
    {
        let Some(stdlib_obj_path) = std::env::var("MOLT_STDLIB_OBJ").ok() else {
            return true;
        };
        shared_stdlib_cache_matches(
            Path::new(&stdlib_obj_path),
            std::env::var("MOLT_STDLIB_CACHE_KEY").ok().as_deref(),
            std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok().as_deref(),
            None,
        )
    }
    #[cfg(not(feature = "native-backend"))]
    {
        false
    }
}

#[cfg(any(unix, test))]
pub(crate) fn backend_ir_document_from_json_path(
    path: &str,
) -> Result<molt_backend::BackendIrDocument, String> {
    let file = File::open(path).map_err(|err| format!("failed to open ir_path {path:?}: {err}"))?;
    serde_json::from_reader(io::BufReader::new(file))
        .map_err(|err| format!("failed to parse ir_path {path:?}: {err}"))
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
pub(crate) enum DaemonCompiledOutput {
    #[cfg(feature = "wasm-backend")]
    Bytes(Arc<[u8]>),
    WrittenToPath,
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
pub(crate) fn insert_daemon_cache_entries(
    cache: &mut DaemonCache,
    cache_key: &str,
    function_cache_key: &str,
    output_bytes: Arc<[u8]>,
) {
    if !cache_key.is_empty() && !function_cache_key.is_empty() && function_cache_key != cache_key {
        cache.insert(cache_key.to_string(), Arc::clone(&output_bytes));
        cache.insert(function_cache_key.to_string(), output_bytes);
    } else if !cache_key.is_empty() {
        cache.insert(cache_key.to_string(), output_bytes);
    } else if !function_cache_key.is_empty() {
        cache.insert(function_cache_key.to_string(), output_bytes);
    }
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
#[cfg(any(unix, test))]
pub(crate) fn maybe_cache_output_file(
    cache: &mut DaemonCache,
    output_path: &Path,
    cache_key: &str,
    function_cache_key: &str,
    warnings: &mut Vec<String>,
) {
    if cache_key.is_empty() && function_cache_key.is_empty() {
        return;
    }
    let metadata = match std::fs::metadata(output_path) {
        Ok(metadata) => metadata,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': metadata failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    let output_len = metadata.len();
    if cache
        .max_bytes
        .is_some_and(|max_bytes| output_len > max_bytes as u64)
    {
        let warning = format!(
            "skipped daemon memory cache for '{}' ({} bytes exceeds cache budget)",
            output_path.display(),
            output_len
        );
        eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
        warnings.push(warning);
        return;
    }
    let bytes = match std::fs::read(output_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let warning = format!(
                "skipped daemon memory cache for '{}': read failed: {err}",
                output_path.display()
            );
            eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
            warnings.push(warning);
            return;
        }
    };
    insert_daemon_cache_entries(
        cache,
        cache_key,
        function_cache_key,
        Arc::from(bytes.into_boxed_slice()),
    );
}

#[cfg(any(unix, test))]
pub(crate) fn compile_single_job(
    job: DaemonJobRequest,
    _cache: &mut DaemonCache,
) -> DaemonJobResponse {
    #[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
    {
        let unsupported = if job.is_wasm {
            "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend"
        } else {
            "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend"
        };
        return DaemonJobResponse {
            id: job.id,
            ok: false,
            cached: false,
            cache_tier: None,
            output_written: false,
            needs_ir: false,
            message: Some(unsupported.to_string()),
            warnings: Vec::new(),
        };
    }

    #[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
    {
        let cache_key = job.cache_key.trim();
        let function_cache_key = job
            .function_cache_key
            .as_deref()
            .map(str::trim)
            .unwrap_or("");
        let daemon_memory_cache_allowed = daemon_memory_cache_allowed_for_job(&job);
        if daemon_memory_cache_allowed
            && !cache_key.is_empty()
            && let Some(bytes) = _cache.get_bytes(cache_key)
        {
            match write_cached_output(&job.output, bytes, job.skip_module_output_if_synced) {
                Ok(output_written) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: true,
                        cached: true,
                        cache_tier: Some("module".to_string()),
                        output_written,
                        needs_ir: false,
                        message: None,
                        warnings: Vec::new(),
                    };
                }
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write cached output: {err}")),
                        warnings: Vec::new(),
                    };
                }
            }
        }
        if daemon_memory_cache_allowed
            && !function_cache_key.is_empty()
            && function_cache_key != cache_key
            && let Some(bytes) = _cache.get_bytes(function_cache_key)
        {
            match write_cached_output(&job.output, bytes, job.skip_function_output_if_synced) {
                Ok(output_written) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: true,
                        cached: true,
                        cache_tier: Some("function".to_string()),
                        output_written,
                        needs_ir: false,
                        message: None,
                        warnings: Vec::new(),
                    };
                }
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write cached output: {err}")),
                        warnings: Vec::new(),
                    };
                }
            }
        }

        if job.probe_cache_only {
            return DaemonJobResponse {
                id: job.id,
                ok: true,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: true,
                message: None,
                warnings: Vec::new(),
            };
        }

        let document = if let Some(document) = job.ir {
            document
        } else if let Some(ir_path) = job.ir_path.as_deref() {
            match backend_ir_document_from_json_path(ir_path) {
                Ok(document) => document,
                Err(err) => {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(err),
                        warnings: Vec::new(),
                    };
                }
            }
        } else {
            return DaemonJobResponse {
                id: job.id,
                ok: false,
                cached: false,
                cache_tier: None,
                output_written: false,
                needs_ir: false,
                message: Some("missing ir for cache miss".to_string()),
                warnings: Vec::new(),
            };
        };
        let molt_backend::BackendIrDocument {
            mut ir,
            module_registry,
        } = document;

        let mut warnings = Vec::new();
        let compiled_output = if job.is_wasm {
            #[cfg(feature = "wasm-backend")]
            {
                let mut options = WasmCompileOptions {
                    reloc_enabled: job.wasm_link,
                    ..WasmCompileOptions::default()
                };
                if let Some(data_base) = job.wasm_data_base {
                    options.data_base = data_base;
                }
                if let Some(table_base) = job.wasm_table_base {
                    options.table_base = table_base;
                }
                if let Some(split_runtime_runtime_table_min) =
                    job.wasm_split_runtime_runtime_table_min
                {
                    options.split_runtime_runtime_table_min = Some(split_runtime_runtime_table_min);
                }
                let backend = WasmBackend::with_options(options);
                DaemonCompiledOutput::Bytes(Arc::from(backend.compile(ir)))
            }
            #[cfg(not(feature = "wasm-backend"))]
            {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    output_written: false,
                    needs_ir: false,
                    message: Some(
                        "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend".to_string(),
                    ),
                    warnings: Vec::new(),
                };
            }
        } else {
            #[cfg(feature = "native-backend")]
            {
                let target_triple = job.target_triple.as_deref();
                let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
                let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
                let expected_stdlib_cache_manifest =
                    std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
                let entry_module =
                    std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
                let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
                let explicit_stdlib_module_symbols =
                    match molt_backend::stdlib_module_symbols_from_env() {
                        Ok(symbols) => symbols,
                        Err(err) => {
                            return DaemonJobResponse {
                                id: job.id,
                                ok: false,
                                cached: false,
                                cache_tier: None,
                                output_written: false,
                                needs_ir: false,
                                message: Some(err),
                                warnings: Vec::new(),
                            };
                        }
                    };
                let compile_options = match prepare_native_application_object(
                    &mut ir,
                    NativeStdlibCachePrepare {
                        target_triple,
                        stdlib_obj_path: stdlib_obj_path.as_deref(),
                        expected_cache_key: expected_stdlib_cache_key.as_deref(),
                        expected_cache_manifest: expected_stdlib_cache_manifest.as_deref(),
                        have_entry_module,
                        entry_module: &entry_module,
                        explicit_stdlib_module_symbols: explicit_stdlib_module_symbols.as_ref(),
                        log_prefix: "MOLT_BACKEND(daemon)",
                        module_registry,
                    },
                ) {
                    Ok(options) => options,
                    Err(err) => {
                        return DaemonJobResponse {
                            id: job.id,
                            ok: false,
                            cached: false,
                            cache_tier: None,
                            output_written: false,
                            needs_ir: false,
                            message: Some(err.to_string()),
                            warnings: Vec::new(),
                        };
                    }
                };

                if let Err(err) = compile_native_application_object_to_path(
                    ir,
                    Path::new(&job.output),
                    compile_options,
                ) {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!(
                            "failed to compile native application object: {err}"
                        )),
                        warnings: Vec::new(),
                    };
                }
                DaemonCompiledOutput::WrittenToPath
            }
            #[cfg(not(feature = "native-backend"))]
            {
                return DaemonJobResponse {
                    id: job.id,
                    ok: false,
                    cached: false,
                    cache_tier: None,
                    output_written: false,
                    needs_ir: false,
                    message: Some(
                        "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend".to_string(),
                    ),
                    warnings: Vec::new(),
                };
            }
        };

        match compiled_output {
            #[cfg(feature = "wasm-backend")]
            DaemonCompiledOutput::Bytes(output_bytes) => {
                if let Err(err) = write_output(&job.output, output_bytes.as_ref()) {
                    return DaemonJobResponse {
                        id: job.id,
                        ok: false,
                        cached: false,
                        cache_tier: None,
                        output_written: false,
                        needs_ir: false,
                        message: Some(format!("failed to write compiled output: {err}")),
                        warnings: Vec::new(),
                    };
                }
                insert_daemon_cache_entries(_cache, cache_key, function_cache_key, output_bytes);
            }
            DaemonCompiledOutput::WrittenToPath => {
                if daemon_memory_cache_allowed {
                    maybe_cache_output_file(
                        _cache,
                        Path::new(&job.output),
                        cache_key,
                        function_cache_key,
                        &mut warnings,
                    );
                }
            }
        }

        DaemonJobResponse {
            id: job.id,
            ok: true,
            cached: false,
            cache_tier: None,
            output_written: true,
            needs_ir: false,
            message: None,
            warnings,
        }
    }
}
