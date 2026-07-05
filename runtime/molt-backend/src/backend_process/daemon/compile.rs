use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "wasm-backend")]
use molt_backend::{WasmBackend, WasmCompileOptions};

use super::super::io_limits::write_output;
#[cfg(feature = "native-backend")]
use super::super::native_batch::compile_native_application_object_to_path;
#[cfg(feature = "native-backend")]
use super::super::shared_stdlib_cache::{
    NativeStdlibCachePrepare, prepare_native_application_object,
};
use super::{
    DaemonCache, DaemonJobRequest, DaemonJobResponse, daemon_memory_cache_allowed_for_job,
    insert_daemon_cache_entries, maybe_cache_output_file, try_write_cached_daemon_job_output,
};

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
            && let Some(response) =
                try_write_cached_daemon_job_output(_cache, &job, cache_key, function_cache_key)
        {
            return response;
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
