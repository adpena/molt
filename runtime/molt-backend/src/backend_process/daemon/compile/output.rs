use std::path::Path;

#[cfg(feature = "wasm-backend")]
use super::super::super::io_limits::write_output;
#[cfg(feature = "wasm-backend")]
use super::super::cache::insert_daemon_cache_entries;
use super::super::cache::{DaemonCache, maybe_cache_output_file};
use super::super::protocol::DaemonJobRequest;
use super::target::DaemonCompiledOutput;

pub(super) fn write_daemon_compiled_output(
    cache: &mut DaemonCache,
    job: &DaemonJobRequest,
    cache_key: &str,
    function_cache_key: &str,
    daemon_memory_cache_allowed: bool,
    compiled_output: DaemonCompiledOutput,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    match compiled_output {
        #[cfg(feature = "wasm-backend")]
        DaemonCompiledOutput::Bytes(output_bytes) => {
            write_output(&job.output, output_bytes.as_ref())
                .map_err(|err| format!("failed to write compiled output: {err}"))?;
            insert_daemon_cache_entries(cache, cache_key, function_cache_key, output_bytes);
        }
        DaemonCompiledOutput::WrittenToPath => {
            if daemon_memory_cache_allowed {
                maybe_cache_output_file(
                    cache,
                    Path::new(&job.output),
                    cache_key,
                    function_cache_key,
                    warnings,
                );
            }
        }
    }
    Ok(())
}
