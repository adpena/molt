mod ir;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
mod target;

use std::path::Path;

use super::super::io_limits::write_output;
use super::{
    DaemonCache, DaemonJobRequest, DaemonJobResponse, daemon_memory_cache_allowed_for_job,
    insert_daemon_cache_entries, maybe_cache_output_file, try_write_cached_daemon_job_output,
};

pub(crate) use ir::backend_ir_document_from_json_path;
#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
use target::{DaemonCompiledOutput, compile_daemon_job_output};

#[cfg(any(unix, test))]
pub(crate) fn compile_single_job(
    mut job: DaemonJobRequest,
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

        let document = if let Some(document) = job.ir.take() {
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

        let mut warnings = Vec::new();
        let compiled_output = match compile_daemon_job_output(&job, document) {
            Ok(output) => output,
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
